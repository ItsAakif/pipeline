use anyhow::{Result, Context};
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use std::path::{Path, PathBuf};
use serde_json::Value;
use object::{Object, ObjectSection};
use crate::compiler::orchestrator::{
    run_capped, unique_temp_dir, CHILD_STDERR_CAP, CHILD_TEXT_CAP,
};
use crate::models::compilation_stages::*;

/// Name of the source file inside the per-request temp directory (F7). Using a
/// relative name + `current_dir` keeps absolute temp paths out of the IR output.
const SRC_FILE_NAME: &str = "source.c";

/// Upper bound on how many decoded instructions are returned (R5).
const MAX_DECODED_INSTRUCTIONS: usize = 5000;

/// Whether `file` (as reported by clang) refers to our per-request source file.
fn is_source_file(file: &str) -> bool {
    file == SRC_FILE_NAME || file.ends_with(&format!("/{}", SRC_FILE_NAME))
}

/// Carry-forward source location state.
///
/// clang's `-ast-dump=json` omits `file`/`line` when they are unchanged from the
/// previously emitted location, so we thread this through the tree (F3).
#[derive(Clone, Default)]
struct LocState {
    file: Option<String>,
    line: u64,
    col: u64,
}

/// Update `state` from a clang location object (`loc`, `range.begin`, ...).
/// Absent fields keep their carried values.
fn apply_loc(state: &mut LocState, loc: Option<&Value>) {
    let Some(loc) = loc else { return };

    if let Some(file) = loc.get("file").and_then(|v| v.as_str()) {
        state.file = Some(file.to_string());
    }
    if let Some(line) = loc.get("line").and_then(|v| v.as_u64()) {
        state.line = line;
    }
    if let Some(col) = loc.get("col").and_then(|v| v.as_u64()) {
        state.col = col;
    }
}

pub struct EducationalCompiler {
    /// `None` when clang was not found at startup (R11). The server still boots;
    /// educational stages simply fail and are omitted from the response.
    clang_path: Option<PathBuf>,
    /// Cap for the captured clang AST JSON (R5).
    max_ast_bytes: usize,
}

impl EducationalCompiler {
    /// Create the educational compiler. Never fails (R11).
    pub fn new(max_ast_bytes: usize) -> Self {
        let clang_path = Self::find_clang();

        if clang_path.is_none() {
            log::warn!(
                "Clang not found; educational compilation stages will be unavailable."
            );
        }

        Self {
            clang_path,
            max_ast_bytes,
        }
    }

    /// Resolve the clang path or return a clear error (R11).
    fn clang(&self) -> Result<&PathBuf> {
        self.clang_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Clang not found"))
    }

    fn find_clang() -> Option<PathBuf> {
        let possible_paths = [
            "/usr/bin/clang",
            "C:\\msys64\\mingw64\\bin\\clang.exe",
            "clang",
        ];

        for path in possible_paths {
            if std::path::Path::new(path).exists() {
                log::info!("Found clang at: {}", path);
                return Some(PathBuf::from(path));
            }
        }

        None
    }

    pub async fn compile_with_stages(
        &self,
        source_code: &str,
        timeout_secs: u64,
        prebuilt_binary: Option<&[u8]>,
    ) -> Result<CompilationStages> {
        // Fail fast with a clear error if no compiler is available.
        let _ = self.clang()?;

        // F7: one temp directory per request containing `source.c`. All clang
        // invocations run with `current_dir` and the relative name so the IR and
        // diagnostics show `source.c`, never an absolute temp path.
        let work_dir = unique_temp_dir("pipeline_edu_work");
        std::fs::create_dir_all(&work_dir)?;

        let outcome = self
            .compile_stages_in_dir(&work_dir, source_code, timeout_secs, prebuilt_binary)
            .await;

        // Always clean up, even on failure.
        let _ = std::fs::remove_dir_all(&work_dir);

        outcome
    }

    async fn compile_stages_in_dir(
        &self,
        work_dir: &Path,
        source_code: &str,
        timeout_secs: u64,
        prebuilt_binary: Option<&[u8]>,
    ) -> Result<CompilationStages> {
        let source_file = work_dir.join(SRC_FILE_NAME);
        std::fs::write(&source_file, source_code)?;

        // Stage 1: Tokenization
        log::info!("Stage 1: Tokenization");
        let tokens = self.extract_tokens(work_dir, timeout_secs).await?;

        // Stage 2: AST
        log::info!("Stage 2: AST Generation");
        let ast = self.extract_ast(work_dir, timeout_secs).await?;

        // Stage 3: Symbol Table (extract from AST)
        log::info!("Stage 3: Symbol Table");
        let symbols = Self::extract_symbols(&ast);

        // Stage 4: LLVM IR (unoptimized)
        log::info!("Stage 4: LLVM IR (unoptimized)");
        let (ir_unoptimized, _) = self.extract_ir(work_dir, timeout_secs, false).await?;

        // Stage 5: LLVM IR (optimized) + real optimization remarks on stderr.
        log::info!("Stage 5: LLVM IR (optimized)");
        let (ir_optimized, o2_remarks) = self.extract_ir(work_dir, timeout_secs, true).await?;

        // Stage 6: Detect optimizations from real remarks + IR (F4)
        let optimizations_applied =
            Self::detect_optimizations(&ir_unoptimized, &ir_optimized, &o2_remarks);

        // Stage 7: Assembly
        log::info!("Stage 6: Assembly Generation");
        let assembly = self.extract_assembly(work_dir, timeout_secs).await?;
        let assembly_annotated = Self::annotate_assembly(&assembly);

        // Stage 8: Binary -- reuse the object file the orchestrator already
        // produced (R4) instead of invoking clang a third time.
        log::info!("Stage 7: Binary Generation");
        let binary = match prebuilt_binary {
            Some(bytes) if !bytes.is_empty() => bytes.to_vec(),
            _ => self.compile_to_binary(work_dir, timeout_secs).await?,
        };

        // Stage 9: Binary Decoding
        log::info!("Stage 8: Binary Decoding");
        let binary_decoded = Self::decode_binary(&binary);

        Ok(CompilationStages {
            tokens,
            ast,
            symbols,
            ir_unoptimized,
            ir_optimized,
            optimizations_applied,
            assembly,
            assembly_annotated,
            binary,
            binary_decoded,
        })
    }

    async fn extract_tokens(&self, work_dir: &Path, timeout_secs: u64) -> Result<Vec<TokenInfo>> {
        // Use clang to dump tokens. `-fsyntax-only` skips codegen (R4).
        // clang writes `-dump-tokens` to stderr, so that stream carries the data.
        let mut cmd = Command::new(self.clang()?);
        cmd.current_dir(work_dir)
            .args(Self::common_clang_args())
            .arg("-fsyntax-only")
            .arg("-Xclang")
            .arg("-dump-tokens")
            .arg(SRC_FILE_NAME);

        let output = timeout(
            Duration::from_secs(timeout_secs),
            run_capped(&mut cmd, CHILD_STDERR_CAP, self.max_ast_bytes),
        )
        .await
        .context("Token extraction timeout")??;

        // Clang writes -dump-tokens to stderr, not stdout
        let tokens_str = String::from_utf8_lossy(&output.stderr);

        // Parse clang token output
        Self::parse_clang_tokens(&tokens_str)
    }

    fn parse_clang_tokens(tokens_str: &str) -> Result<Vec<TokenInfo>> {
        use regex::Regex;
        let mut tokens = Vec::new();

        // Regex to capture filename and position: Loc=<filename:line:col>
        let token_regex = Regex::new(
            r"(?P<type>\w+)\s+'(?P<value>[^']*)'\s+.*?Loc=<(?P<file>[^:]+):(?P<line>\d+):(?P<col>\d+)>"
        ).context("Failed to compile token regex")?;

        for line in tokens_str.lines() {
            if line.trim().is_empty() || line.starts_with("clang") {
                continue;
            }

            if let Some(caps) = token_regex.captures(line) {
                let file = caps.name("file").unwrap().as_str();

                // ONLY include tokens from our source file, skip system headers
                if !is_source_file(file) {
                    continue;
                }

                let token_type = caps.name("type").unwrap().as_str().to_string();
                let value = caps.name("value").unwrap().as_str().to_string();
                let line_num: usize = caps.name("line").unwrap().as_str().parse()
                    .context("Failed to parse line number")?;
                let col_num: usize = caps.name("col").unwrap().as_str().parse()
                    .context("Failed to parse column number")?;
                let value_len = value.len();

                let explanation = Self::explain_token(&token_type, &value);

                tokens.push(TokenInfo {
                    token_type,
                    value,
                    line: line_num,
                    column: col_num,
                    length: value_len,
                    explanation,
                });
            }
        }

        Ok(tokens)
    }

    fn explain_token(token_type: &str, value: &str) -> String {
        match token_type {
            "int" | "char" | "float" | "double" | "void" => {
                format!("Keyword: '{}' is a data type that tells the compiler what kind of value a variable holds.", value)
            }
            "return" => {
                "Keyword: 'return' exits a function and optionally sends a value back to the caller.".to_string()
            }
            "identifier" => {
                format!("Identifier: '{}' is a name you chose for a variable, function, or other program element.", value)
            }
            "numeric_constant" => {
                format!("Literal: '{}' is a number written directly in your code.", value)
            }
            "l_paren" => "Punctuation: '(' opens a parameter list or groups expressions.".to_string(),
            "r_paren" => "Punctuation: ')' closes a parameter list or grouped expression.".to_string(),
            "l_brace" => "Punctuation: '{' starts a code block (function body, loop, etc.).".to_string(),
            "r_brace" => "Punctuation: '}' ends a code block.".to_string(),
            "semi" => "Punctuation: ';' marks the end of a statement.".to_string(),
            _ => format!("Token: '{}'", value),
        }
    }

    async fn extract_ast(&self, work_dir: &Path, timeout_secs: u64) -> Result<Option<ASTRepresentation>> {
        // The AST JSON is the single largest clang output. Read stdout with a
        // hard cap of MAX_AST_BYTES so the child cannot make us buffer more.
        let mut cmd = Command::new(self.clang()?);
        cmd.current_dir(work_dir)
            .args(Self::common_clang_args())
            .arg("-Xclang")
            .arg("-ast-dump=json")
            .arg("-fsyntax-only")
            .arg(SRC_FILE_NAME);

        let output = timeout(
            Duration::from_secs(timeout_secs),
            run_capped(&mut cmd, self.max_ast_bytes, CHILD_STDERR_CAP),
        )
        .await
        .context("AST extraction timeout")??;

        if !output.status.success() {
            log::warn!("AST generation failed: {}", String::from_utf8_lossy(&output.stderr));
            return Ok(None);
        }

        if output.truncated {
            log::warn!(
                "AST output exceeded the {} byte read cap; output was truncated",
                self.max_ast_bytes
            );
        }

        // R5: bound the captured AST JSON so a pathological dump cannot exhaust
        // memory. Truncate on a char boundary so the string stays valid UTF-8.
        // (Second guard: the read cap above already limits what we buffer.)
        let mut ast_json = String::from_utf8_lossy(&output.stdout).to_string();
        if ast_json.len() > self.max_ast_bytes {
            log::warn!(
                "AST JSON exceeds MAX_AST_BYTES ({} > {}); truncating",
                ast_json.len(),
                self.max_ast_bytes
            );
            let mut end = self.max_ast_bytes;
            while end > 0 && !ast_json.is_char_boundary(end) {
                end -= 1;
            }
            ast_json.truncate(end);
        }

        log::info!("AST JSON (first 500 chars): {}", ast_json.chars().take(500).collect::<String>());

        // Parse JSON AST
        let root = Self::parse_ast_json(&ast_json)?;
        log::info!("AST parsed successfully, root type: {}", root.node_type);

        Ok(Some(ASTRepresentation {
            root,
            visualization_data: ast_json,
        }))
    }

    fn parse_ast_json(json_str: &str) -> Result<ASTNode> {
        let ast_json: Value = serde_json::from_str(json_str)
            .context("Failed to parse AST JSON")?;

        let mut state = LocState::default();
        Self::parse_ast_node(&ast_json, &mut state)
            .ok_or_else(|| anyhow::anyhow!("AST root node has no kind"))
    }

    /// Parse one clang AST node (F3, F5).
    ///
    /// Returns `None` for placeholder entries that have no `kind` (clang emits
    /// these inside e.g. `ForStmt`), so the UI never shows an "Unknown" node.
    fn parse_ast_node(node: &Value, state: &mut LocState) -> Option<ASTNode> {
        let kind = node["kind"].as_str()?;

        // F3: carry forward the source location.
        apply_loc(state, node.get("loc"));

        // F5: variable references carry their name in `referencedDecl`.
        let name = node["name"]
            .as_str()
            .or_else(|| {
                node.get("referencedDecl")
                    .and_then(|decl| decl.get("name"))
                    .and_then(|v| v.as_str())
            })
            .map(String::from);

        // Extract source range (also carries file/line forward).
        let range = if let Some(range_obj) = node.get("range") {
            Self::parse_source_range(range_obj, state)
        } else {
            SourceRange {
                start_line: state.line as usize,
                start_column: state.col as usize,
                end_line: state.line as usize,
                end_column: state.col as usize,
            }
        };

        // Parse children recursively (in emission order, so carry-forward works).
        let mut children = Vec::new();
        if let Some(inner) = node["inner"].as_array() {
            for child in inner {
                if kind == "TranslationUnitDecl" {
                    // F3: filter by the *carried* file, not a directly-read loc.file.
                    apply_loc(state, child.get("loc"));
                    let child_file = state.file.clone();
                    if let Some(child_node) = Self::parse_ast_node(child, state) {
                        let from_source = child_file
                            .as_deref()
                            .map(is_source_file)
                            .unwrap_or(false);
                        if from_source {
                            children.push(child_node);
                        }
                    }
                } else if let Some(child_node) = Self::parse_ast_node(child, state) {
                    children.push(child_node);
                }
            }
        }

        let explanation = Self::explain_ast_node(kind, &name);

        // Extract opcode for operators
        let opcode = node.get("opcode").and_then(|v| v.as_str()).map(String::from);

        Some(ASTNode {
            node_type: kind.to_string(),
            name,
            value: node["value"].as_str().map(String::from),
            opcode,
            source_range: range,
            children,
            explanation,
        })
    }

    fn parse_source_range(range: &Value, state: &mut LocState) -> SourceRange {
        apply_loc(state, range.get("begin"));
        let start_line = state.line as usize;
        let start_column = state.col as usize;

        apply_loc(state, range.get("end"));
        let end_line = state.line as usize;
        let end_column = state.col as usize;

        SourceRange {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    fn explain_ast_node(kind: &str, name: &Option<String>) -> String {
        match kind {
            // Top-level declarations
            "TranslationUnitDecl" => "Translation Unit - root of entire AST".to_string(),
            "FunctionDecl" => format!("Function Declaration: {}", name.as_deref().unwrap_or("<anonymous>")),
            "VarDecl" => format!("Variable Declaration: {}", name.as_deref().unwrap_or("<unnamed>")),
            "ParmVarDecl" => format!("Parameter: {}", name.as_deref().unwrap_or("<unnamed>")),
            "TypedefDecl" => format!("Type Definition: {}", name.as_deref().unwrap_or("<unnamed>")),

            // Statements
            "CompoundStmt" => "Compound Statement { ... }".to_string(),
            "DeclStmt" => "Declaration Statement".to_string(),
            "ReturnStmt" => "Return Statement".to_string(),
            "IfStmt" => "If Statement".to_string(),
            "WhileStmt" => "While Loop".to_string(),
            "ForStmt" => "For Loop".to_string(),
            "DoStmt" => "Do-While Loop".to_string(),
            "BreakStmt" => "Break Statement".to_string(),
            "ContinueStmt" => "Continue Statement".to_string(),
            "NullStmt" => "Empty Statement ;".to_string(),

            // Expressions
            "IntegerLiteral" => "Integer Literal".to_string(),
            "FloatingLiteral" => "Floating-Point Literal".to_string(),
            "StringLiteral" => "String Literal".to_string(),
            "CharacterLiteral" => "Character Literal".to_string(),

            // Operators
            "BinaryOperator" => "Binary Operator".to_string(),
            "UnaryOperator" => "Unary Operator".to_string(),
            "CompoundAssignOperator" => "Compound Assignment (+=, -=, etc)".to_string(),

            // References and calls
            "CallExpr" => "Function Call".to_string(),
            "DeclRefExpr" => format!("Variable Reference{}", name.as_ref().map(|n| format!(": {}", n)).unwrap_or_default()),

            // Casts (important in AST!)
            "ImplicitCastExpr" => "Implicit Type Cast (compiler-inserted)".to_string(),
            "CStyleCastExpr" => "C-Style Cast (type)expr".to_string(),

            // Other expressions
            "ParenExpr" => "Parenthesized Expression".to_string(),
            "ArraySubscriptExpr" => "Array Subscript []".to_string(),
            "MemberExpr" => "Member Access . or ->".to_string(),
            "ConditionalOperator" => "Ternary Operator ? :".to_string(),
            "UnaryExprOrTypeTraitExpr" => "sizeof/alignof".to_string(),

            _ => kind.to_string(),
        }
    }

    fn extract_symbols(ast: &Option<ASTRepresentation>) -> SymbolTableInfo {
        let mut scopes = Vec::new();

        if let Some(ast_repr) = ast {
            log::info!("Extracting symbols from AST with root type: {}", ast_repr.root.node_type);
            let mut global_symbols = Vec::new();
            Self::walk_ast_for_symbols(&ast_repr.root, &mut global_symbols, "global", &mut scopes);

            log::info!("Found {} global symbols and {} scopes", global_symbols.len(), scopes.len());
            scopes.insert(0, Scope {
                name: "global".to_string(),
                parent: None,
                symbols: global_symbols,
            });
        } else {
            log::warn!("No AST available for symbol extraction");
            scopes.push(Scope {
                name: "global".to_string(),
                parent: None,
                symbols: vec![],
            });
        }

        SymbolTableInfo { scopes }
    }

    fn walk_ast_for_symbols(
        node: &ASTNode,
        current_symbols: &mut Vec<Symbol>,
        scope_name: &str,
        all_scopes: &mut Vec<Scope>,
    ) {
        match node.node_type.as_str() {
            "FunctionDecl" => {
                if let Some(ref name) = node.name {
                    current_symbols.push(Symbol {
                        name: name.clone(),
                        symbol_type: SymbolType::Function,
                        data_type: "function".to_string(),
                        line_declared: node.source_range.start_line,
                        explanation: format!("Function '{}' defined at line {}", name, node.source_range.start_line),
                    });

                    // Create new scope for function body
                    let mut func_scope_symbols = Vec::new();
                    for child in &node.children {
                        Self::walk_ast_for_symbols(child, &mut func_scope_symbols, name, all_scopes);
                    }

                    if !func_scope_symbols.is_empty() {
                        all_scopes.push(Scope {
                            name: name.clone(),
                            parent: Some(scope_name.to_string()),
                            symbols: func_scope_symbols,
                        });
                    }
                }
            }
            "VarDecl" => {
                if let Some(ref name) = node.name {
                    current_symbols.push(Symbol {
                        name: name.clone(),
                        symbol_type: SymbolType::Variable,
                        data_type: "int".to_string(),
                        line_declared: node.source_range.start_line,
                        explanation: format!("Local variable '{}' declared at line {}", name, node.source_range.start_line),
                    });
                }
            }
            "ParmVarDecl" => {
                if let Some(ref name) = node.name {
                    current_symbols.push(Symbol {
                        name: name.clone(),
                        symbol_type: SymbolType::Parameter,
                        data_type: "int".to_string(),
                        line_declared: node.source_range.start_line,
                        explanation: format!("Parameter '{}' for this function", name),
                    });
                }
            }
            _ => {
                // Recursively walk children
                for child in &node.children {
                    Self::walk_ast_for_symbols(child, current_symbols, scope_name, all_scopes);
                }
            }
        }
    }

    /// Arguments applied to every clang invocation.
    ///
    /// `-fdebug-compilation-dir=.` keeps the working directory out of any
    /// emitted debug metadata so no absolute temp path leaks into the IR.
    fn common_clang_args() -> Vec<String> {
        vec!["-fdebug-compilation-dir=.".to_string()]
    }

    /// Arguments for the IR stage (`-S -emit-llvm`).
    ///
    /// For the optimized run, `-Rpass` is a **single combined regex**: repeated
    /// `-Rpass=` flags do not accumulate (the last one wins), so separate flags
    /// would leave only one pass active.
    fn ir_stage_args(optimized: bool) -> Vec<String> {
        let mut args = vec![
            "-S".to_string(),
            "-emit-llvm".to_string(),
            "--target=x86_64-unknown-linux-gnu".to_string(),
            "-fdebug-compilation-dir=.".to_string(),
        ];

        if optimized {
            args.push("-O2".to_string());
            args.push("-Rpass=(loop-unroll|inline|loop-vectorize|slp-vectorizer)".to_string());
        } else {
            args.push("-O0".to_string());
        }

        args
    }

    async fn extract_ir(
        &self,
        work_dir: &Path,
        timeout_secs: u64,
        optimized: bool,
    ) -> Result<(String, String)> {
        let mut cmd = Command::new(self.clang()?);
        cmd.current_dir(work_dir).args(Self::ir_stage_args(optimized));
        cmd.arg("-o").arg("-").arg(SRC_FILE_NAME);

        let output = timeout(
            Duration::from_secs(timeout_secs),
            run_capped(&mut cmd, CHILD_TEXT_CAP, CHILD_STDERR_CAP),
        )
        .await
        .context("IR extraction timeout")??;

        if !output.status.success() {
            return Err(anyhow::anyhow!("IR generation failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        Ok((
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }

    /// Parse `remark: <msg> [-Rpass=<pass>]` lines emitted by `-Rpass`.
    fn parse_rpass_remarks(stderr: &str) -> Vec<(String, String)> {
        let mut remarks: Vec<(String, String)> = Vec::new();

        for line in stderr.lines() {
            let Some(remark_pos) = line.find("remark:") else { continue };
            let after = line[remark_pos + "remark:".len()..].trim_start();
            let Some(marker_pos) = after.find("[-Rpass=") else { continue };

            let message = after[..marker_pos].trim();
            let rest = &after[marker_pos + "[-Rpass=".len()..];
            let Some(end) = rest.find(']') else { continue };
            let pass = rest[..end].trim();

            if message.is_empty() || pass.is_empty() {
                continue;
            }

            let entry = (pass.to_string(), message.to_string());
            if !remarks.contains(&entry) {
                remarks.push(entry);
            }
        }

        remarks
    }

    fn friendly_optimization(pass: &str, message: &str) -> String {
        let name = match pass {
            "loop-unroll" => "Loop unrolling",
            "inline" => "Function inlining",
            "loop-vectorize" => "Loop vectorization",
            "slp-vectorizer" => "SLP vectorization",
            "mem2reg" => "Register promotion (mem2reg)",
            "licm" => "Loop-invariant code motion",
            "gvn" => "Global value numbering",
            "instcombine" => "Instruction combining",
            "tailcallelim" => "Tail-call elimination",
            other => other,
        };

        format!("{}: {}", name, message)
    }

    /// Report only real, verifiable optimizations (F4).
    ///
    /// Sources: clang `-Rpass` remarks from the O2 run, plus two defensible IR
    /// checks. If nothing is detected we return an empty list rather than
    /// inventing labels.
    fn detect_optimizations(unoptimized_ir: &str, optimized_ir: &str, o2_stderr: &str) -> Vec<String> {
        let mut applied: Vec<String> = Vec::new();

        for (pass, message) in Self::parse_rpass_remarks(o2_stderr) {
            let label = Self::friendly_optimization(&pass, &message);
            if !applied.contains(&label) {
                applied.push(label);
            }
        }

        if optimized_ir.contains("tail call") {
            let label = "Tail-call optimization: a call in tail position was rewritten as a jump".to_string();
            if !applied.contains(&label) {
                applied.push(label);
            }
        }

        if unoptimized_ir.contains("alloca") && !optimized_ir.contains("alloca") {
            let label = "Register promotion (mem2reg): stack variables promoted to SSA registers".to_string();
            if !applied.contains(&label) {
                applied.push(label);
            }
        }

        applied
    }

    async fn extract_assembly(&self, work_dir: &Path, timeout_secs: u64) -> Result<String> {
        let mut cmd = Command::new(self.clang()?);
        cmd.current_dir(work_dir)
            .args(Self::common_clang_args())
            .arg("-S")
            .arg("-masm=intel")
            .arg("-O2")
            .arg("--target=x86_64-unknown-linux-gnu")
            .arg("-o")
            .arg("-")
            .arg(SRC_FILE_NAME);

        let output = timeout(
            Duration::from_secs(timeout_secs),
            run_capped(&mut cmd, CHILD_TEXT_CAP, CHILD_STDERR_CAP),
        )
        .await
        .context("Assembly extraction timeout")??;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Assembly generation failed"));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn annotate_assembly(assembly: &str) -> Vec<AnnotatedInstruction> {
        let mut annotated = Vec::new();
        let mut instruction_number = 0;

        for line in assembly.lines() {
            let line = line.trim();

            // Skip empty lines, directives, and comments
            if line.is_empty() || line.starts_with('.') || line.starts_with('#') {
                continue;
            }

            // Skip labels (lines containing : before any whitespace, or ending with :)
            if line.contains(':') {
                let before_space = line.split_whitespace().next().unwrap_or("");
                if before_space.ends_with(':') {
                    continue;
                }
            }

            // Parse instruction
            if let Some((mnemonic, operands)) = line.split_once(char::is_whitespace) {
                let mnemonic = mnemonic.trim();
                // Skip if it looks like a label
                if mnemonic.ends_with(':') {
                    continue;
                }

                let (simple, detailed) = Self::explain_instruction(mnemonic);
                instruction_number += 1;

                annotated.push(AnnotatedInstruction {
                    address: 0,
                    bytes: vec![],
                    mnemonic: mnemonic.to_string(),
                    operands: operands.trim().to_string(),
                    full_instruction: line.to_string(),
                    explanation_simple: simple,
                    explanation_detailed: detailed,
                    affects_flags: vec![],
                    reads_registers: vec![],
                    writes_registers: vec![],
                    memory_access: None,
                    line_number: instruction_number,
                });
            } else if !line.is_empty() && !line.ends_with(':') {
                // Single word instruction (like "ret")
                let (simple, detailed) = Self::explain_instruction(line);
                instruction_number += 1;

                annotated.push(AnnotatedInstruction {
                    address: 0,
                    bytes: vec![],
                    mnemonic: line.to_string(),
                    operands: String::new(),
                    full_instruction: line.to_string(),
                    explanation_simple: simple,
                    explanation_detailed: detailed,
                    affects_flags: vec![],
                    reads_registers: vec![],
                    writes_registers: vec![],
                    memory_access: None,
                    line_number: instruction_number,
                });
            }
        }

        annotated
    }

    /// Beginner/advanced explanation for an Intel-syntax mnemonic (F6).
    fn explain_instruction(mnemonic: &str) -> (String, String) {
        match mnemonic.to_lowercase().as_str() {
            "mov" => (
                "MOV copies a value from one place to another.".to_string(),
                "MOV is the fundamental data-transfer instruction: it copies the source operand into the destination without changing the source. Neither the flags nor the source are modified.".to_string(),
            ),
            "movabs" => (
                "MOVABS loads a full 64-bit constant into a register.".to_string(),
                "MOVABS is the 64-bit form of MOV with a 64-bit immediate (B8+rd with REX.W). Used when a constant does not fit in 32 bits.".to_string(),
            ),
            "movzx" => (
                "MOVZX copies a value into a wider register, filling the new high bits with zeros.".to_string(),
                "MOVZX zero-extends an 8- or 16-bit source into a larger destination (e.g. unsigned char to int). The upper bits become 0; flags are unaffected.".to_string(),
            ),
            "movsx" => (
                "MOVSX copies a value into a wider register, repeating its sign bit.".to_string(),
                "MOVSX sign-extends an 8- or 16-bit source into a larger destination (e.g. signed char to int), replicating the sign bit into the upper bits. Flags are unaffected.".to_string(),
            ),
            "movsxd" => (
                "MOVSXD sign-extends a 32-bit value to 64 bits.".to_string(),
                "MOVSXD (MOVSX with a dword source) widens a signed 32-bit value to 64 bits by replicating bit 31. This is how the compiler widens an `int` to a pointer-sized value. Flags are unaffected.".to_string(),
            ),
            "lea" => (
                "LEA computes an address without reading memory.".to_string(),
                "LEA (Load Effective Address) evaluates the memory operand's address and stores it in the destination register. It never accesses memory, so it is also used for cheap constant arithmetic. Flags are unaffected.".to_string(),
            ),
            "push" => (
                "PUSH saves a value onto the stack.".to_string(),
                "PUSH decrements RSP by the operand size and writes the value to the new stack top. Used to preserve registers across calls or to pass arguments. Flags are unaffected.".to_string(),
            ),
            "pop" => (
                "POP retrieves a value from the stack.".to_string(),
                "POP reads the value at the current stack top (RSP) and increments RSP by the operand size. Used to restore saved registers. Flags are unaffected.".to_string(),
            ),
            "add" => (
                "ADD adds two values.".to_string(),
                "ADD computes dest = dest + src, storing the result in the destination and updating the flags (ZF, SF, CF, OF).".to_string(),
            ),
            "sub" => (
                "SUB subtracts one value from another.".to_string(),
                "SUB computes dest = dest - src, storing the result in the destination and updating the flags (ZF, SF, CF, OF).".to_string(),
            ),
            "inc" => (
                "INC increases a value by 1.".to_string(),
                "INC adds 1 to the operand and updates ZF, SF and OF (but leaves CF unchanged).".to_string(),
            ),
            "dec" => (
                "DEC decreases a value by 1.".to_string(),
                "DEC subtracts 1 from the operand and updates ZF, SF and OF (but leaves CF unchanged).".to_string(),
            ),
            "imul" => (
                "IMUL performs signed multiplication.".to_string(),
                "IMUL multiplies two signed operands. In the two/three-operand form it stores the low half of the product in the destination and sets CF/OF when the result does not fit.".to_string(),
            ),
            "mul" => (
                "MUL performs unsigned multiplication.".to_string(),
                "MUL multiplies RAX by the operand and stores the result in RDX:RAX. CF/OF are set if the high half is non-zero.".to_string(),
            ),
            "div" => (
                "DIV performs unsigned division.".to_string(),
                "DIV divides RDX:RAX by the operand; the quotient goes to RAX and the remainder to RDX. Division by zero raises #DE.".to_string(),
            ),
            "idiv" => (
                "IDIV performs signed division.".to_string(),
                "IDIV divides RDX:RAX by the signed operand; the quotient goes to RAX and the remainder to RDX. Division by zero raises #DE.".to_string(),
            ),
            "neg" => (
                "NEG negates a value (two's complement).".to_string(),
                "NEG computes dest = 0 - dest, i.e. flips the sign of a two's-complement number. It updates ZF, SF, CF (set unless the input was 0) and OF.".to_string(),
            ),
            "and" => (
                "AND performs a bitwise AND.".to_string(),
                "AND computes dest = dest & src bit by bit. It clears OF and CF and updates ZF and SF. Often used to mask bits or test for zero.".to_string(),
            ),
            "or" => (
                "OR performs a bitwise OR.".to_string(),
                "OR computes dest = dest | src bit by bit. It clears OF and CF and updates ZF and SF. Often used to set bits.".to_string(),
            ),
            "xor" => (
                "XOR performs a bitwise exclusive-OR.".to_string(),
                "XOR sets each result bit to 1 when the operand bits differ, 0 when they match. It clears OF and CF and updates ZF and SF. XOR reg, reg is the idiomatic way to zero a register.".to_string(),
            ),
            "not" => (
                "NOT inverts every bit of the operand.".to_string(),
                "NOT performs a bitwise one's complement (dest = ~dest). Unlike NEG it does not affect any flags.".to_string(),
            ),
            "cmp" => (
                "CMP compares two values by subtracting them.".to_string(),
                "CMP computes dest - src purely to set the flags (ZF, SF, CF, OF); it does not store the result. It is normally followed by a conditional jump.".to_string(),
            ),
            "test" => (
                "TEST performs a bitwise AND to set flags.".to_string(),
                "TEST computes dest & src purely to set the flags (ZF, SF; CF and OF are cleared) without storing the result. TEST reg, reg tests whether a register is zero.".to_string(),
            ),
            "shl" | "sal" => (
                "SHL shifts the operand left.".to_string(),
                "SHL/SAL shifts the bits left by the count operand, filling with zeros. It updates CF (last bit shifted out), ZF, SF and OF.".to_string(),
            ),
            "shr" => (
                "SHR shifts the operand right, filling with zeros.".to_string(),
                "SHR is a logical right shift: bits move right and the vacated high bits become 0. It updates CF, ZF and SF.".to_string(),
            ),
            "sar" => (
                "SAR shifts the operand right, preserving the sign.".to_string(),
                "SAR is an arithmetic right shift: bits move right and the vacated high bits are filled with the sign bit, so negative numbers stay negative. It updates CF, ZF and SF.".to_string(),
            ),
            "jmp" => (
                "JMP jumps to another instruction unconditionally.".to_string(),
                "JMP transfers control to the target address with no condition. The target is encoded as a relative displacement.".to_string(),
            ),
            "je" | "jz" => (
                "JE/JZ jumps when the previous comparison was equal.".to_string(),
                "JE (alias JZ) branches when the Zero Flag (ZF) is set, i.e. when the last result was zero or two values compared equal. Used after CMP or TEST.".to_string(),
            ),
            "jne" | "jnz" => (
                "JNE/JNZ jumps when the previous comparison was not equal.".to_string(),
                "JNE (alias JNZ) branches when the Zero Flag (ZF) is clear, i.e. when the last result was non-zero. Used after CMP or TEST.".to_string(),
            ),
            "jg" => (
                "JG jumps when the first value is greater (signed).".to_string(),
                "JG branches when ZF=0 and SF=OF, i.e. a signed greater-than comparison. Used after CMP.".to_string(),
            ),
            "jge" => (
                "JGE jumps when the first value is greater or equal (signed).".to_string(),
                "JGE branches when SF=OF, i.e. a signed greater-or-equal comparison. Used after CMP.".to_string(),
            ),
            "jl" => (
                "JL jumps when the first value is less (signed).".to_string(),
                "JL branches when SF!=OF, i.e. a signed less-than comparison. Used after CMP.".to_string(),
            ),
            "jle" => (
                "JLE jumps when the first value is less or equal (signed).".to_string(),
                "JLE branches when ZF=1 or SF!=OF, i.e. a signed less-or-equal comparison. Used after CMP.".to_string(),
            ),
            "ja" => (
                "JA jumps when the first value is above (unsigned).".to_string(),
                "JA branches when CF=0 and ZF=0, i.e. an unsigned greater-than comparison.".to_string(),
            ),
            "jb" => (
                "JB jumps when the first value is below (unsigned).".to_string(),
                "JB branches when CF=1, i.e. an unsigned less-than comparison.".to_string(),
            ),
            "call" => (
                "CALL invokes a function.".to_string(),
                "CALL pushes the address of the next instruction onto the stack (the return address) and transfers control to the target function.".to_string(),
            ),
            "ret" => (
                "RET returns from a function.".to_string(),
                "RET pops the return address from the stack into RIP, transferring control back to the caller. The caller's frame is restored by the matching epilogue.".to_string(),
            ),
            "leave" => (
                "LEAVE tears down a stack frame.".to_string(),
                "LEAVE sets RSP = RBP (discarding the frame) and then pops the saved RBP. It is the standard epilogue for a function that used a frame pointer.".to_string(),
            ),
            "nop" => (
                "NOP does nothing.".to_string(),
                "NOP is a one-byte no-operation instruction, used for padding or alignment. It has no effect on registers, memory or flags.".to_string(),
            ),
            "endbr64" => (
                "ENDBR64 marks a valid indirect-branch target.".to_string(),
                "ENDBR64 terminates an indirect branch (CET/IBT marker). It is a no-op on CPUs without CET and exists so the CPU can validate indirect jumps and calls.".to_string(),
            ),
            "cdqe" => (
                "CDQE sign-extends EAX into RAX.".to_string(),
                "CDQE (Convert Doubleword to Quadword Extend) sign-extends the 32-bit EAX into the 64-bit RAX by replicating bit 31. It takes no operands and affects no flags.".to_string(),
            ),
            "cqo" => (
                "CQO sign-extends RAX into RDX:RAX.".to_string(),
                "CQO copies the sign bit of RAX into every bit of RDX, forming the 128-bit dividend needed before a signed IDIV.".to_string(),
            ),
            "cdq" => (
                "CDQ sign-extends EAX into EDX:EAX.".to_string(),
                "CDQ copies the sign bit of EAX into every bit of EDX, preparing a signed 64-bit dividend for IDIV.".to_string(),
            ),
            "js" => (
                "JS jumps when the result was negative.".to_string(),
                "JS branches when the Sign Flag (SF) is set, i.e. the last result had its high bit set.".to_string(),
            ),
            "jns" => (
                "JNS jumps when the result was non-negative.".to_string(),
                "JNS branches when the Sign Flag (SF) is clear, i.e. the last result had its high bit clear.".to_string(),
            ),
            "sete" | "setz" => (
                "SETE/SETZ sets a byte to 1 when equal/zero, else 0.".to_string(),
                "SETE (alias SETZ) writes 1 to the destination byte when the Zero Flag is set, otherwise 0. Used to materialize a boolean comparison result.".to_string(),
            ),
            "setne" | "setnz" => (
                "SETNE/SETNZ sets a byte to 1 when not equal, else 0.".to_string(),
                "SETNE (alias SETNZ) writes 1 to the destination byte when the Zero Flag is clear, otherwise 0.".to_string(),
            ),
            "setl" => (
                "SETL sets a byte to 1 when the first value was less (signed).".to_string(),
                "SETL writes 1 when SF!=OF (signed less-than), otherwise 0.".to_string(),
            ),
            "setle" => (
                "SETLE sets a byte to 1 when less or equal (signed).".to_string(),
                "SETLE writes 1 when ZF=1 or SF!=OF (signed less-or-equal), otherwise 0.".to_string(),
            ),
            "setg" => (
                "SETG sets a byte to 1 when greater (signed).".to_string(),
                "SETG writes 1 when ZF=0 and SF=OF (signed greater-than), otherwise 0.".to_string(),
            ),
            "setge" => (
                "SETGE sets a byte to 1 when greater or equal (signed).".to_string(),
                "SETGE writes 1 when SF=OF (signed greater-or-equal), otherwise 0.".to_string(),
            ),
            "cmove" | "cmovz" => (
                "CMOVE conditionally copies a value when equal/zero.".to_string(),
                "CMOVE (alias CMOVZ) copies the source into the destination only when the Zero Flag is set; otherwise the destination is unchanged.".to_string(),
            ),
            "cmovne" | "cmovnz" => (
                "CMOVNE conditionally copies a value when not equal.".to_string(),
                "CMOVNE (alias CMOVNZ) copies the source into the destination only when the Zero Flag is clear.".to_string(),
            ),
            "cmovl" => (
                "CMOVL conditionally copies a value when less (signed).".to_string(),
                "CMOVL copies the source when SF!=OF (signed less-than).".to_string(),
            ),
            "cmovle" => (
                "CMOVLE conditionally copies a value when less or equal (signed).".to_string(),
                "CMOVLE copies the source when ZF=1 or SF!=OF.".to_string(),
            ),
            "cmovg" => (
                "CMOVG conditionally copies a value when greater (signed).".to_string(),
                "CMOVG copies the source when ZF=0 and SF=OF.".to_string(),
            ),
            "cmovge" => (
                "CMOVGE conditionally copies a value when greater or equal (signed).".to_string(),
                "CMOVGE copies the source when SF=OF.".to_string(),
            ),
            "xchg" => (
                "XCHG swaps two values.".to_string(),
                "XCHG exchanges the contents of its two operands. It is atomic when a memory operand is involved (an implicit LOCK). Flags are unaffected.".to_string(),
            ),
            _ => (
                format!("{} performs a machine operation.", mnemonic),
                format!("The {} instruction executes directly on the CPU; see an x86-64 reference for its precise semantics.", mnemonic),
            ),
        }
    }

    async fn compile_to_binary(&self, work_dir: &Path, timeout_secs: u64) -> Result<Vec<u8>> {
        let output_name = "output.o";
        let output_file = work_dir.join(output_name);

        let mut cmd = Command::new(self.clang()?);
        cmd.current_dir(work_dir)
            .args(Self::common_clang_args())
            .arg("-c")
            .arg(SRC_FILE_NAME)
            .arg("-o")
            .arg(output_name)
            .arg("-O2")
            .arg("--target=x86_64-unknown-linux-gnu");

        let output = timeout(
            Duration::from_secs(timeout_secs),
            run_capped(&mut cmd, CHILD_STDERR_CAP, CHILD_STDERR_CAP),
        )
        .await
        .context("Binary compilation timeout")??;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Binary compilation failed"));
        }

        let binary = std::fs::read(&output_file)?;

        Ok(binary)
    }

    fn decode_binary(binary: &[u8]) -> Vec<DecodedInstruction> {
        use iced_x86::{Decoder, DecoderOptions, Instruction as IcedInstruction};

        let mut decoded_instructions = Vec::new();

        if binary.is_empty() {
            return decoded_instructions;
        }

        // F1: decode ONLY the real `.text` section. If we cannot find it, return
        // an empty list -- never fall back to decoding the whole ELF file.
        let (text_offset, text_section) = match Self::extract_text_section(binary) {
            Some(found) => found,
            None => {
                log::warn!("No .text section found; skipping binary decode");
                return decoded_instructions;
            }
        };

        if text_section.is_empty() {
            return decoded_instructions;
        }

        // The decoder runs over the section (IP 0 = section-relative, matching
        // `objdump -d`); reported offsets are file offsets into the object.
        let mut decoder = Decoder::with_ip(64, text_section, 0x00000000, DecoderOptions::NONE);
        let mut instruction = IcedInstruction::default();

        while decoder.can_decode() {
            // R5: cap the number of decoded instructions we keep in memory.
            if decoded_instructions.len() >= MAX_DECODED_INSTRUCTIONS {
                log::warn!(
                    "Reached decoded-instruction cap ({}); truncating",
                    MAX_DECODED_INSTRUCTIONS
                );
                break;
            }

            decoder.decode_out(&mut instruction);

            if instruction.is_invalid() {
                break;
            }

            let section_offset = instruction.ip() as usize;
            let length = instruction.len();
            let bytes = match text_section.get(section_offset..section_offset + length) {
                Some(b) => b.to_vec(),
                None => break,
            };
            let offset = text_offset + section_offset;

            // F2: accurate encoding decomposition straight from iced-x86.
            let offsets = decoder.get_constant_offsets(&instruction);
            let breakdown = Self::decompose_encoding(&instruction, &bytes, &offsets);

            // Sanity: prefix + opcode + modrm + sib + disp + imm must equal len.
            if breakdown.accounted_bytes() != length {
                log::debug!(
                    "Encoding breakdown for {:?} accounts for {} of {} bytes",
                    instruction.mnemonic(),
                    breakdown.accounted_bytes(),
                    length
                );
            }

            let explanation = Self::explain_instruction_encoding(
                &instruction,
                &breakdown.prefixes,
                breakdown.modrm.as_ref(),
                breakdown.sib.as_ref(),
            );

            decoded_instructions.push(DecodedInstruction {
                offset,
                bytes,
                encoding: InstructionEncoding {
                    prefixes: breakdown.prefixes,
                    opcode: breakdown.opcode,
                    modrm: breakdown.modrm,
                    sib: breakdown.sib,
                    displacement: breakdown.displacement,
                    immediate: breakdown.immediate,
                },
                explanation,
            });
        }

        decoded_instructions
    }

    /// Extract the bytes of the `.text` section and its file offset (F1).
    ///
    /// Prefers the `object` crate; falls back to a hand-rolled ELF32/ELF64
    /// section-header parse. Returns `None` when `.text` cannot be located --
    /// callers must never decode the whole file in that case.
    fn extract_text_section(binary: &[u8]) -> Option<(usize, &[u8])> {
        // Preferred: the `object` crate handles ELF/PE/Mach-O and validates the
        // section table.
        if let Ok(file) = object::File::parse(binary) {
            if let Some(section) = file.section_by_name(".text") {
                if let Ok(data) = section.data() {
                    let file_offset = section.file_range().map(|(offset, _)| offset as usize).unwrap_or(0);
                    return Some((file_offset, data));
                }
            }
        }

        // Fallback: hand-rolled ELF64/ELF32 parse.
        Self::extract_text_section_elf(binary)
    }

    /// Hand-rolled ELF section-header parse for `.text`.
    fn extract_text_section_elf(binary: &[u8]) -> Option<(usize, &[u8])> {
        if binary.len() < 64 || &binary[0..4] != b"\x7fELF" {
            return None;
        }

        let is64 = match binary[4] {
            1 => false,
            2 => true,
            _ => return None,
        };

        let (shoff, shentsize, shnum, shstrndx) = if is64 {
            (
                read_u64(binary, 0x28)? as usize,
                read_u16(binary, 0x3A)? as usize,
                read_u16(binary, 0x3C)? as usize,
                read_u16(binary, 0x3E)? as usize,
            )
        } else {
            (
                read_u32(binary, 0x20)? as usize,
                read_u16(binary, 0x2E)? as usize,
                read_u16(binary, 0x30)? as usize,
                read_u16(binary, 0x32)? as usize,
            )
        };

        let min_entsize = if is64 { 64 } else { 40 };
        if shnum == 0 || shentsize < min_entsize || shstrndx >= shnum {
            return None;
        }

        let section_header = |index: usize| -> Option<(u32, usize, usize)> {
            let base = shoff.checked_add(index.checked_mul(shentsize)?)?;
            if base.checked_add(shentsize)? > binary.len() {
                return None;
            }

            let name = read_u32(binary, base)?;
            let (offset, size) = if is64 {
                (read_u64(binary, base + 24)? as usize, read_u64(binary, base + 32)? as usize)
            } else {
                (read_u32(binary, base + 16)? as usize, read_u32(binary, base + 20)? as usize)
            };
            Some((name, offset, size))
        };

        let (_, strtab_offset, strtab_size) = section_header(shstrndx)?;
        let strtab = binary.get(strtab_offset..strtab_offset.checked_add(strtab_size)?)?;

        for index in 0..shnum {
            let Some((name_offset, offset, size)) = section_header(index) else { continue };
            let Some(name) = cstr(strtab, name_offset as usize) else { continue };
            if name == b".text" {
                let data = binary.get(offset..offset.checked_add(size)?)?;
                return Some((offset, data));
            }
        }

        None
    }

    /// `true` if the instruction encodes a ModR/M byte, per iced-x86's operand
    /// kinds (F2).
    fn has_modrm(instruction: &iced_x86::Instruction) -> bool {
        use iced_x86::OpCodeOperandKind as K;

        instruction.op_code().op_kinds().iter().any(|kind| {
            matches!(
                kind,
                // Memory (modrm)
                K::mem
                    | K::mem_mpx
                    | K::mem_mib
                    | K::mem_vsib32x
                    | K::mem_vsib64x
                    | K::mem_vsib32y
                    | K::mem_vsib64y
                    | K::mem_vsib32z
                    | K::mem_vsib64z
                    | K::sibmem
                    // GPR or memory
                    | K::r8_or_mem
                    | K::r16_or_mem
                    | K::r32_or_mem
                    | K::r32_or_mem_mpx
                    | K::r64_or_mem
                    | K::r64_or_mem_mpx
                    // SIMD / MMX / mask / bounds register or memory
                    | K::mm_or_mem
                    | K::xmm_or_mem
                    | K::ymm_or_mem
                    | K::zmm_or_mem
                    | K::k_or_mem
                    | K::bnd_or_mem_mpx
                    // GPR encoded in the modrm `reg` field
                    | K::r8_reg
                    | K::r16_reg
                    | K::r16_reg_mem
                    | K::r32_reg
                    | K::r32_reg_mem
                    | K::r64_reg
                    | K::r64_reg_mem
                    // GPR encoded in the modrm `mod + r/m` fields
                    | K::r16_rm
                    | K::r32_rm
                    | K::r64_rm
                    // SIMD / MMX / mask / tile / special registers in `reg` / `mod + r/m`
                    | K::mm_reg
                    | K::mm_rm
                    | K::xmm_reg
                    | K::xmm_rm
                    | K::ymm_reg
                    | K::ymm_rm
                    | K::zmm_reg
                    | K::zmm_rm
                    | K::k_reg
                    | K::kp1_reg
                    | K::k_rm
                    | K::tmm_reg
                    | K::tmm_rm
                    | K::bnd_reg
                    | K::cr_reg
                    | K::dr_reg
                    | K::tr_reg
                    | K::seg_reg
            )
        })
    }

    /// Pure, testable byte-level decomposition of one decoded instruction (F2).
    fn decompose_encoding(
        instruction: &iced_x86::Instruction,
        bytes: &[u8],
        offsets: &iced_x86::ConstantOffsets,
    ) -> EncodingBreakdown {
        let len = bytes.len();
        let prefix_len = Self::count_prefixes(bytes);
        let has_modrm = Self::has_modrm(instruction);
        let has_displacement = offsets.has_displacement();
        let has_immediate = offsets.has_immediate();

        let displacement_offset = if has_displacement { offsets.displacement_offset() } else { usize::MAX };
        let immediate_offset = if has_immediate { offsets.immediate_offset() } else { usize::MAX };

        // The byte after the opcode (+ modrm/sib) is the displacement if present,
        // else the immediate, else end-of-instruction.
        let anchor = if has_displacement {
            displacement_offset
        } else if has_immediate {
            immediate_offset
        } else {
            len
        };

        let mut modrm = None;
        let mut sib = None;
        let opcode: Vec<u8>;

        if has_modrm {
            // Locate the ModR/M byte.
            //
            // For legacy encodings the opcode length is known exactly
            // (`op_code_len`), so the ModR/M sits at `prefix_len + opcode_len`.
            // This avoids a false positive in the anchor heuristic for 2-byte
            // opcodes whose second byte looks like a ModR/M with `rm==4`
            // (e.g. `0f 94` = sete). VEX/EVEX/XOP carry extra prefix bytes, so
            // those fall back to the anchor heuristic.
            let legacy = matches!(
                instruction.op_code().encoding(),
                iced_x86::EncodingKind::Legacy
            );

            let (modrm_index, sib_index) = if legacy {
                let index = prefix_len + Self::legacy_opcode_byte_len(instruction.op_code());
                let has_sib = bytes
                    .get(index)
                    .map(|&b| (b & 0x07) == 4 && ((b >> 6) & 0x03) != 3)
                    .unwrap_or(false);
                (index, if has_sib { Some(index + 1) } else { None })
            } else {
                let sib_present = anchor >= 2
                    && anchor - 2 > prefix_len
                    && anchor - 2 < len
                    && (bytes[anchor - 2] & 0x07) == 4
                    && ((bytes[anchor - 2] >> 6) & 0x03) != 3;

                if sib_present {
                    (anchor - 2, Some(anchor - 1))
                } else {
                    (anchor.wrapping_sub(1), None)
                }
            };

            if modrm_index >= prefix_len && modrm_index < len {
                modrm = Some(Self::make_modrm(bytes[modrm_index]));
                if let Some(sib_index) = sib_index {
                    if sib_index < len {
                        sib = Some(Self::make_sib(bytes[sib_index]));
                    }
                }
                opcode = bytes[prefix_len..modrm_index].to_vec();
            } else {
                let end = anchor.min(len);
                opcode = bytes[prefix_len.min(end)..end].to_vec();
            }
        } else {
            let opcode_end = displacement_offset.min(immediate_offset).min(len);
            opcode = bytes[prefix_len.min(opcode_end)..opcode_end].to_vec();
        }

        // Values come from iced-x86 (no byte guessing).
        let displacement = if has_displacement {
            Some(instruction.memory_displacement64() as i32)
        } else {
            None
        };

        let immediate = if has_immediate {
            Some(Self::read_signed(bytes, immediate_offset, offsets.immediate_size()))
        } else {
            None
        };

        EncodingBreakdown {
            prefixes: Self::parse_prefixes(bytes, prefix_len),
            opcode,
            modrm,
            sib,
            displacement,
            displacement_size: if has_displacement { offsets.displacement_size() } else { 0 },
            immediate,
            immediate_size: if has_immediate { offsets.immediate_size() } else { 0 },
        }
    }

    /// Number of opcode bytes in the instruction stream for a legacy encoding.
    ///
    /// iced's `op_code_len()` excludes the escape byte(s) (which live in
    /// `table()`), so a `0F xx` instruction reports length 1 for the opcode.
    fn legacy_opcode_byte_len(op: &iced_x86::OpCodeInfo) -> usize {
        use iced_x86::OpCodeTableKind as T;

        let escape = match op.table() {
            T::Normal => 0,
            T::T0F => 1,
            T::T0F38 | T::T0F3A => 2,
            _ => 0,
        };

        op.op_code_len() as usize + escape
    }

    fn count_prefixes(bytes: &[u8]) -> usize {
        let mut count = 0;
        while count < bytes.len() && is_legacy_prefix(bytes[count]) {
            count += 1;
        }
        if count < bytes.len() && (0x40..=0x4F).contains(&bytes[count]) {
            count += 1;
        }
        count
    }

    fn parse_prefixes(bytes: &[u8], prefix_len: usize) -> Vec<Prefix> {
        let mut prefixes = Vec::new();

        for &byte in bytes.iter().take(prefix_len) {
            let (name, purpose) = match byte {
                0x66 => ("Operand-size override".to_string(), "Switches the operand size (e.g. 32-bit to 16-bit)"),
                0x67 => ("Address-size override".to_string(), "Switches the address size"),
                0xF0 => ("LOCK".to_string(), "Makes a read-modify-write memory operation atomic"),
                0xF2 => ("REPNE/REPNZ".to_string(), "Repeats a string operation while not equal/zero"),
                0xF3 => ("REP/REPE/REPZ".to_string(), "Repeats a string operation"),
                0x2E => ("CS segment override".to_string(), "Overrides the default segment register with CS"),
                0x36 => ("SS segment override".to_string(), "Overrides the default segment register with SS"),
                0x3E => ("DS segment override".to_string(), "Overrides the default segment register with DS"),
                0x26 => ("ES segment override".to_string(), "Overrides the default segment register with ES"),
                0x64 => ("FS segment override".to_string(), "Overrides the default segment register with FS"),
                0x65 => ("GS segment override".to_string(), "Overrides the default segment register with GS"),
                0x40..=0x4F => (Self::rex_name(byte), "Extends registers / selects 64-bit operand size"),
                _ => (format!("0x{:02X}", byte), "Instruction prefix"),
            };

            prefixes.push(Prefix {
                byte,
                name,
                purpose: purpose.to_string(),
            });
        }

        prefixes
    }

    fn rex_name(byte: u8) -> String {
        let mut name = String::from("REX.");
        if byte & 0x08 != 0 {
            name.push('W');
        }
        if byte & 0x04 != 0 {
            name.push('R');
        }
        if byte & 0x02 != 0 {
            name.push('X');
        }
        if byte & 0x01 != 0 {
            name.push('B');
        }
        if name == "REX." {
            name = "REX".to_string();
        }
        name
    }

    fn make_modrm(byte: u8) -> ModRMByte {
        let mode = (byte >> 6) & 0x03;
        let reg = (byte >> 3) & 0x07;
        let rm = byte & 0x07;

        ModRMByte {
            byte,
            mode,
            reg,
            rm,
            explanation: format!("mod={}, reg={}, r/m={} - selects the register/memory operands", mode, reg, rm),
        }
    }

    fn make_sib(byte: u8) -> SIBByte {
        let scale = (byte >> 6) & 0x03;
        let index = (byte >> 3) & 0x07;
        let base = byte & 0x07;

        SIBByte {
            byte,
            scale,
            index,
            base,
            explanation: format!("scale={}, index={}, base={} - scaled-index memory addressing", 1 << scale, index, base),
        }
    }

    /// Read `size` little-endian bytes at `offset` and sign-extend to `i64`.
    fn read_signed(bytes: &[u8], offset: usize, size: usize) -> i64 {
        let mut value: u64 = 0;
        for i in 0..size.min(8) {
            if let Some(&byte) = bytes.get(offset + i) {
                value |= (byte as u64) << (8 * i);
            }
        }

        match size {
            1 => value as u8 as i8 as i64,
            2 => value as u16 as i16 as i64,
            4 => value as u32 as i32 as i64,
            _ => value as i64,
        }
    }

    fn explain_instruction_encoding(
        instruction: &iced_x86::Instruction,
        prefixes: &[Prefix],
        modrm: Option<&ModRMByte>,
        sib: Option<&SIBByte>,
    ) -> String {
        use iced_x86::Mnemonic;

        let mut explanation = format!("{:?}: ", instruction.mnemonic());

        if !prefixes.is_empty() {
            explanation.push_str(&format!("{} prefix(es) modify the instruction behavior. ", prefixes.len()));
        }

        match instruction.mnemonic() {
            Mnemonic::Push => explanation.push_str("Decrements stack pointer and stores value."),
            Mnemonic::Pop => explanation.push_str("Loads value from stack and increments pointer."),
            Mnemonic::Mov => explanation.push_str("Copies data from source to destination."),
            Mnemonic::Add => explanation.push_str("Adds source to destination."),
            Mnemonic::Sub => explanation.push_str("Subtracts source from destination."),
            Mnemonic::Call => explanation.push_str("Pushes return address and jumps to function."),
            Mnemonic::Ret => explanation.push_str("Returns from function by popping return address."),
            Mnemonic::Xor => explanation.push_str("Bitwise XOR operation."),
            Mnemonic::Lea => explanation.push_str("Loads effective address (calculates pointer)."),
            Mnemonic::Cmp => explanation.push_str("Compares operands and sets flags."),
            Mnemonic::Test => explanation.push_str("Bitwise AND for flags only."),
            Mnemonic::Jmp => explanation.push_str("Unconditional jump to the target."),
            _ => explanation.push_str("Executes the encoded operation."),
        }

        if modrm.is_some() {
            explanation.push_str(" Uses ModR/M byte to specify register/memory operands.");
        }

        if sib.is_some() {
            explanation.push_str(" Uses SIB byte for complex memory addressing.");
        }

        explanation
    }
}

/// Byte-level decomposition of a single decoded instruction (F2).
#[derive(Debug, Clone)]
struct EncodingBreakdown {
    prefixes: Vec<Prefix>,
    opcode: Vec<u8>,
    modrm: Option<ModRMByte>,
    sib: Option<SIBByte>,
    displacement: Option<i32>,
    displacement_size: usize,
    immediate: Option<i64>,
    immediate_size: usize,
}

impl EncodingBreakdown {
    /// Total bytes accounted for. Must equal the instruction length.
    fn accounted_bytes(&self) -> usize {
        self.prefixes.len()
            + self.opcode.len()
            + usize::from(self.modrm.is_some())
            + usize::from(self.sib.is_some())
            + self.displacement_size
            + self.immediate_size
    }
}

fn is_legacy_prefix(byte: u8) -> bool {
    matches!(
        byte,
        0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3 | 0x2E | 0x36 | 0x3E | 0x26 | 0x64 | 0x65
    )
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?))
}

/// Read a NUL-terminated byte string at `offset`.
fn cstr(bytes: &[u8], offset: usize) -> Option<&[u8]> {
    let rest = bytes.get(offset..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_x86::{Decoder, DecoderOptions};

    fn decompose(bytes: &[u8]) -> (usize, EncodingBreakdown) {
        let mut decoder = Decoder::with_ip(64, bytes, 0, DecoderOptions::NONE);
        let instruction = decoder.decode();
        let offsets = decoder.get_constant_offsets(&instruction);
        let len = instruction.len();
        let breakdown = EducationalCompiler::decompose_encoding(&instruction, bytes, &offsets);
        (len, breakdown)
    }

    #[test]
    fn encoding_movaps_xmm_xmm() {
        // 0f 28 c1 -> movaps xmm0,xmm1 (xmm_reg + xmm_or_mem)
        let (len, b) = decompose(&[0x0f, 0x28, 0xc1]);
        assert_eq!(len, 3);
        assert!(b.prefixes.is_empty());
        assert_eq!(b.opcode, vec![0x0f, 0x28]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!((m.byte, m.mode, m.reg, m.rm), (0xc1, 3, 0, 1));
        assert!(b.sib.is_none());
        assert_eq!(b.displacement_size, 0);
        assert_eq!(b.immediate_size, 0);
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_movdqu_xmm_mem_has_sib() {
        // f3 0f 6f 04 24 -> movdqu xmm0,[rsp]
        let (len, b) = decompose(&[0xf3, 0x0f, 0x6f, 0x04, 0x24]);
        assert_eq!(len, 5);
        assert_eq!(b.prefixes.len(), 1, "F3 prefix");
        assert_eq!(b.prefixes[0].byte, 0xf3);
        assert_eq!(b.opcode, vec![0x0f, 0x6f]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!(m.byte, 0x04);
        let s = b.sib.as_ref().expect("sib");
        assert_eq!(s.byte, 0x24);
        assert_eq!(b.displacement_size, 0);
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_movups_mem_xmm() {
        // 0f 11 07 -> movups [rdi],xmm0
        let (len, b) = decompose(&[0x0f, 0x11, 0x07]);
        assert_eq!(len, 3);
        assert!(b.prefixes.is_empty());
        assert_eq!(b.opcode, vec![0x0f, 0x11]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!(m.byte, 0x07);
        assert!(b.sib.is_none());
        assert_eq!(b.displacement_size, 0);
        assert_eq!(b.immediate_size, 0);
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_movhlps_xmm_rm_has_modrm() {
        // 0f 12 c1 -> movhlps xmm0,xmm1. Its operand kinds are only
        // xmm_reg + xmm_rm, so the reg/rm SIMD variants must be recognized.
        let (len, b) = decompose(&[0x0f, 0x12, 0xc1]);
        assert_eq!(len, 3);
        assert_eq!(b.opcode, vec![0x0f, 0x12]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!((m.byte, m.mode, m.reg, m.rm), (0xc1, 3, 0, 1));
        assert!(b.sib.is_none());
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_mov_rdi_rbx() {
        // 48 89 df -> mov rdi,rbx
        let (len, b) = decompose(&[0x48, 0x89, 0xdf]);
        assert_eq!(len, 3);
        assert_eq!(b.prefixes.len(), 1, "one REX prefix");
        assert_eq!(b.prefixes[0].byte, 0x48);
        assert_eq!(b.opcode, vec![0x89]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!((m.byte, m.mode, m.reg, m.rm), (0xdf, 3, 3, 7));
        assert!(b.sib.is_none());
        assert_eq!(b.displacement_size, 0);
        assert_eq!(b.immediate_size, 0);
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_xor_eax_eax() {
        // 31 c0 -> xor eax,eax
        let (len, b) = decompose(&[0x31, 0xc0]);
        assert_eq!(len, 2);
        assert!(b.prefixes.is_empty());
        assert_eq!(b.opcode, vec![0x31]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!((m.byte, m.mode, m.reg, m.rm), (0xc0, 3, 0, 0));
        assert!(b.sib.is_none());
        assert_eq!(b.displacement_size, 0);
        assert_eq!(b.immediate_size, 0);
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_call_rel32_has_no_modrm() {
        // e8 00 00 00 00 -> call rel32
        let (len, b) = decompose(&[0xe8, 0, 0, 0, 0]);
        assert_eq!(len, 5);
        assert!(b.prefixes.is_empty());
        assert_eq!(b.opcode, vec![0xe8]);
        assert!(b.modrm.is_none(), "call has no ModR/M byte");
        assert!(b.sib.is_none());
        assert_eq!(b.displacement_size, 0);
        assert_eq!(b.immediate_size, 4, "trailing rel32");
        assert_eq!(b.prefixes.len() + b.opcode.len(), 1, "rel32 starts at offset 1");
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_mov_edi_imm32_has_no_modrm() {
        // bf 0a 00 00 00 -> mov edi,0xa
        let (len, b) = decompose(&[0xbf, 0x0a, 0, 0, 0]);
        assert_eq!(len, 5);
        assert!(b.prefixes.is_empty());
        assert_eq!(b.opcode, vec![0xbf]);
        assert!(b.modrm.is_none(), "mov r32,imm32 has no ModR/M byte");
        assert!(b.sib.is_none());
        assert_eq!(b.immediate, Some(10));
        assert_eq!(b.immediate_size, 4);
        assert_eq!(b.prefixes.len() + b.opcode.len(), 1, "imm32 starts at offset 1");
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_lea_rip_relative() {
        // 48 8d 1d 00 00 00 00 -> lea rbx,[rip+0]
        let (len, b) = decompose(&[0x48, 0x8d, 0x1d, 0, 0, 0, 0]);
        assert_eq!(len, 7);
        assert_eq!(b.prefixes.len(), 1);
        assert_eq!(b.opcode, vec![0x8d]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!(m.byte, 0x1d);
        assert!(b.sib.is_none());
        // iced resolves RIP-relative displacements to the absolute target
        // address (next_ip + disp = 7 + 0 here); we report it verbatim.
        assert_eq!(b.displacement, Some(7));
        assert_eq!(b.displacement_size, 4);
        assert_eq!(b.prefixes.len() + b.opcode.len() + 1, 3, "disp32 starts at offset 3");
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_push_rbx() {
        // 53 -> push rbx
        let (len, b) = decompose(&[0x53]);
        assert_eq!(len, 1);
        assert!(b.prefixes.is_empty());
        assert_eq!(b.opcode, vec![0x53]);
        assert!(b.modrm.is_none());
        assert!(b.sib.is_none());
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_mov_rax_rsp_has_sib() {
        // 48 8b 04 24 -> mov rax,[rsp]
        let (len, b) = decompose(&[0x48, 0x8b, 0x04, 0x24]);
        assert_eq!(len, 4);
        assert_eq!(b.prefixes.len(), 1);
        assert_eq!(b.opcode, vec![0x8b]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!(m.byte, 0x04);
        let s = b.sib.as_ref().expect("sib");
        assert_eq!(s.byte, 0x24);
        assert_eq!(b.displacement_size, 0);
        assert_eq!(b.immediate_size, 0);
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_two_byte_opcode_setcc() {
        // 0f 94 c0 -> sete al. The second opcode byte (0x94) looks like a
        // ModR/M with rm==4; it must NOT be mistaken for a ModR/M + SIB.
        let (len, b) = decompose(&[0x0f, 0x94, 0xc0]);
        assert_eq!(len, 3);
        assert!(b.prefixes.is_empty());
        assert_eq!(b.opcode, vec![0x0f, 0x94]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!(m.byte, 0xc0);
        assert!(b.sib.is_none());
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn encoding_two_byte_opcode_imul() {
        // 48 0f af c0 -> imul rax,rbx
        let (len, b) = decompose(&[0x48, 0x0f, 0xaf, 0xc0]);
        assert_eq!(len, 4);
        assert_eq!(b.prefixes.len(), 1);
        assert_eq!(b.opcode, vec![0x0f, 0xaf]);
        let m = b.modrm.as_ref().expect("modrm");
        assert_eq!(m.byte, 0xc0);
        assert!(b.sib.is_none());
        assert_eq!(b.accounted_bytes(), len);
    }

    #[test]
    fn detect_optimizations_parses_and_maps_remarks() {
        let stderr = "source.c:5:5: remark: completely unrolled loop with 5 iterations [-Rpass=loop-unroll]\n\
                      source.c:5:5: remark: completely unrolled loop with 4 iterations [-Rpass=loop-unroll]\n\
                      source.c:5:5: remark: completely unrolled loop with 5 iterations [-Rpass=loop-unroll]\n";

        let remarks = EducationalCompiler::parse_rpass_remarks(stderr);
        assert_eq!(remarks.len(), 2, "duplicates are removed");

        let opts = EducationalCompiler::detect_optimizations("", "", stderr);
        assert!(opts.iter().any(|o| o == "Loop unrolling: completely unrolled loop with 5 iterations"));
        assert_eq!(opts.iter().filter(|o| o.contains("Loop unrolling")).count(), 2);
    }

    #[test]
    fn detect_optimizations_from_ir_only() {
        let unoptimized = "define i32 @f() {\n  %1 = alloca i32\n  ret i32 0\n}";
        let optimized = "define i32 @f() {\n  tail call i32 @g()\n  ret i32 0\n}";

        let opts = EducationalCompiler::detect_optimizations(unoptimized, optimized, "");
        assert!(opts.iter().any(|o| o.contains("Tail-call")), "tail call detected");
        assert!(opts.iter().any(|o| o.contains("Register promotion")), "mem2reg detected");
    }

    #[test]
    fn detect_optimizations_empty_when_nothing_found() {
        let ir = "define i32 @f() {\n  ret i32 0\n}";
        assert!(EducationalCompiler::detect_optimizations(ir, ir, "").is_empty());
    }

    #[test]
    fn optimized_ir_uses_single_combined_rpass_regex() {
        let args = EducationalCompiler::ir_stage_args(true);

        // Exactly one `-Rpass` argument: repeated flags do not accumulate.
        let rpass: Vec<&String> = args.iter().filter(|a| a.starts_with("-Rpass")).collect();
        assert_eq!(rpass.len(), 1, "must not pass four separate -Rpass flags");
        assert_eq!(
            rpass[0],
            "-Rpass=(loop-unroll|inline|loop-vectorize|slp-vectorizer)"
        );

        // The individual flags are gone.
        assert!(!args.iter().any(|a| a == "-Rpass=loop-unroll"));
        assert!(!args.iter().any(|a| a == "-Rpass=slp-vectorizer"));

        assert!(args.contains(&"-O2".to_string()));
        assert!(args.contains(&"-fdebug-compilation-dir=.".to_string()));
    }

    #[test]
    fn unoptimized_ir_has_no_rpass_and_uses_o0() {
        let args = EducationalCompiler::ir_stage_args(false);
        assert!(args.contains(&"-O0".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("-Rpass")));
        assert!(args.contains(&"-fdebug-compilation-dir=.".to_string()));
    }

    #[test]
    fn clang_args_hide_the_temp_dir() {
        let expected = "-fdebug-compilation-dir=.".to_string();
        assert!(EducationalCompiler::common_clang_args().contains(&expected));
        assert!(EducationalCompiler::ir_stage_args(true).contains(&expected));
        assert!(EducationalCompiler::ir_stage_args(false).contains(&expected));
    }

    #[test]
    fn source_file_detection() {
        assert!(is_source_file("source.c"));
        assert!(is_source_file("/tmp/pipeline_edu_work_1_2/source.c"));
        assert!(!is_source_file("/usr/include/stdint.h"));
    }

    /// Minimal synthetic ELF64 with a `.text` section and a `.shstrtab`.
    fn synthetic_elf64() -> (Vec<u8>, Vec<u8>) {
        let text: Vec<u8> = vec![0x53, 0xc3]; // push rbx; ret

        // Section header string table: index 0 = "", then ".shstrtab", ".text".
        let mut strtab: Vec<u8> = Vec::new();
        strtab.push(0);
        let shstrtab_name = strtab.len(); // 1
        strtab.extend_from_slice(b".shstrtab\0");
        let text_name = strtab.len(); // 11
        strtab.extend_from_slice(b".text\0");

        let text_off = 64usize;
        let strtab_off = text_off + text.len();
        let shoff = (strtab_off + strtab.len() + 7) & !7;
        let shentsize = 64usize;
        let shnum = 3usize;

        let mut buf = vec![0u8; shoff + shnum * shentsize];

        // ELF header
        buf[0..4].copy_from_slice(b"\x7fELF");
        buf[4] = 2; // 64-bit
        buf[5] = 1; // little-endian
        buf[6] = 1; // version
        buf[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        buf[0x3A..0x3C].copy_from_slice(&(shentsize as u16).to_le_bytes());
        buf[0x3C..0x3E].copy_from_slice(&(shnum as u16).to_le_bytes());
        buf[0x3E..0x40].copy_from_slice(&1u16.to_le_bytes()); // shstrndx = 1

        // Section data
        buf[text_off..text_off + text.len()].copy_from_slice(&text);
        buf[strtab_off..strtab_off + strtab.len()].copy_from_slice(&strtab);

        // Section header 1: .shstrtab
        let sh1 = shoff + shentsize;
        buf[sh1..sh1 + 4].copy_from_slice(&(shstrtab_name as u32).to_le_bytes());
        buf[sh1 + 4..sh1 + 8].copy_from_slice(&3u32.to_le_bytes()); // SHT_STRTAB
        buf[sh1 + 24..sh1 + 32].copy_from_slice(&(strtab_off as u64).to_le_bytes());
        buf[sh1 + 32..sh1 + 40].copy_from_slice(&(strtab.len() as u64).to_le_bytes());

        // Section header 2: .text
        let sh2 = shoff + 2 * shentsize;
        buf[sh2..sh2 + 4].copy_from_slice(&(text_name as u32).to_le_bytes());
        buf[sh2 + 4..sh2 + 8].copy_from_slice(&1u32.to_le_bytes()); // SHT_PROGBITS
        buf[sh2 + 24..sh2 + 32].copy_from_slice(&(text_off as u64).to_le_bytes());
        buf[sh2 + 32..sh2 + 40].copy_from_slice(&(text.len() as u64).to_le_bytes());

        (buf, text)
    }

    #[test]
    fn extract_text_section_returns_only_text() {
        let (elf, text) = synthetic_elf64();

        // Hand-rolled fallback parser: returns the .text file offset + bytes.
        assert_eq!(
            EducationalCompiler::extract_text_section_elf(&elf),
            Some((64, text.as_slice()))
        );

        // Public helper (object crate, or fallback).
        let (offset, found) = EducationalCompiler::extract_text_section(&elf).expect(".text found");
        assert_eq!(offset, 0x40, "file offset of .text");
        assert_eq!(found, text.as_slice());
        assert_ne!(found.len(), elf.len(), "must not return the whole ELF");
    }

    #[test]
    fn extract_text_section_non_elf_is_none() {
        assert!(EducationalCompiler::extract_text_section(b"this is not an ELF file").is_none());
        assert!(EducationalCompiler::extract_text_section(&[]).is_none());
    }

    #[test]
    fn decode_binary_empty_when_no_text_section() {
        // Non-ELF input must decode to nothing, never the whole buffer.
        let decoded = EducationalCompiler::decode_binary(b"not an object file");
        assert!(decoded.is_empty());
    }

    #[test]
    fn decode_binary_decodes_only_text() {
        let (elf, _text) = synthetic_elf64();
        let decoded = EducationalCompiler::decode_binary(&elf);
        // The synthetic .text is `push rbx; ret` at file offset 0x40.
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].offset, 0x40, "first instruction is at .text");
        assert_eq!(decoded[0].bytes, vec![0x53]);
        assert_eq!(decoded[0].encoding.opcode, vec![0x53]);
        assert!(decoded[0].encoding.modrm.is_none());
        assert_eq!(decoded[1].offset, 0x41);
        assert_eq!(decoded[1].bytes, vec![0xc3]);
    }

    #[test]
    fn parse_ast_carries_lines_and_skips_unknown_kinds() {
        // Mirrors clang's behaviour: `line` is omitted when unchanged, and a
        // `ForStmt` may contain placeholder entries with no `kind`.
        let json = r#"{
            "kind": "TranslationUnitDecl",
            "loc": { "offset": 0, "file": "source.c" },
            "inner": [
                {
                    "kind": "FunctionDecl",
                    "name": "main",
                    "loc": { "offset": 10, "line": 3, "col": 1, "file": "source.c" },
                    "range": { "begin": { "offset": 10, "col": 1 }, "end": { "offset": 90, "line": 12, "col": 1 } },
                    "inner": [
                        { "kind": "CompoundStmt", "loc": { "offset": 20, "line": 3, "col": 1 },
                          "range": { "begin": { "offset": 20, "col": 1 }, "end": { "offset": 80, "col": 1 } },
                          "inner": [
                              { "loc": { "offset": 25, "col": 5 }, "inner": [] },
                              { "kind": "DeclStmt", "loc": { "offset": 30, "line": 4, "col": 5 },
                                "range": { "begin": { "offset": 30, "col": 5 }, "end": { "offset": 40, "col": 6 } },
                                "inner": [
                                    { "kind": "VarDecl", "name": "n",
                                      "loc": { "offset": 30, "col": 5 },
                                      "range": { "begin": { "offset": 30, "col": 5 }, "end": { "offset": 40, "col": 6 } },
                                      "inner": [] }
                                ] }
                          ] }
                    ]
                },
                { "loc": { "offset": 95, "col": 1 }, "inner": [] }
            ]
        }"#;

        let root = EducationalCompiler::parse_ast_json(json).expect("parse");
        assert_eq!(root.node_type, "TranslationUnitDecl");
        // The two placeholder (no-kind) children are skipped, not "Unknown".
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].node_type, "FunctionDecl");

        let compound = &root.children[0].children[0];
        assert_eq!(compound.node_type, "CompoundStmt");
        assert_eq!(compound.source_range.start_line, 3);

        let decl_stmt = &compound.children[0];
        assert_eq!(decl_stmt.node_type, "DeclStmt", "placeholder child skipped");
        assert_eq!(decl_stmt.source_range.start_line, 4);

        let var = &decl_stmt.children[0];
        assert_eq!(var.node_type, "VarDecl");
        assert_eq!(var.name.as_deref(), Some("n"));
        // VarDecl carries `line` forward (its own loc/range omit it).
        assert_eq!(var.source_range.start_line, 4, "carried line number, not 0");
    }

    #[test]
    fn parse_ast_uses_referenced_decl_name() {
        let json = r#"{
            "kind": "TranslationUnitDecl",
            "loc": { "offset": 0, "file": "source.c" },
            "inner": [
                { "kind": "DeclRefExpr", "loc": { "offset": 10, "line": 5, "col": 3 },
                  "referencedDecl": { "kind": "VarDecl", "name": "counter" }, "inner": [] }
            ]
        }"#;

        let root = EducationalCompiler::parse_ast_json(json).expect("parse");
        let decl_ref = &root.children[0];
        assert_eq!(decl_ref.node_type, "DeclRefExpr");
        assert_eq!(decl_ref.name.as_deref(), Some("counter"));
    }

    #[test]
    fn common_mnemonics_have_real_explanations() {
        for mnemonic in [
            "lea", "movzx", "movsx", "movsxd", "test", "cmp", "jmp", "jne", "je", "jg",
            "jle", "jge", "jl", "add", "sub", "imul", "and", "or", "not", "neg", "shl",
            "shr", "sar", "nop", "endbr64", "leave", "cdqe", "xor", "push", "pop", "call",
            "ret", "mov", "movabs", "inc", "dec", "mul", "div", "idiv", "cdq", "cqo",
            "js", "jns", "sete", "setne", "setl", "setle", "setg", "setge", "cmove",
            "cmovne", "cmovl", "cmovle", "cmovg", "cmovge", "xchg", "ja", "jb", "jz", "jnz",
        ] {
            let (simple, detailed) = EducationalCompiler::explain_instruction(mnemonic);
            assert!(!simple.is_empty(), "{mnemonic} simple explanation");
            assert!(!detailed.is_empty(), "{mnemonic} detailed explanation");
            assert!(
                !detailed.starts_with(&format!("{} instruction", mnemonic)),
                "{mnemonic} must not use the generic fallback"
            );
        }
    }
}
