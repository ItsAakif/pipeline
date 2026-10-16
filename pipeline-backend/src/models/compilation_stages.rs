use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilationStages {
    pub tokens: Vec<TokenInfo>,
    pub ast: Option<ASTRepresentation>,
    pub symbols: SymbolTableInfo,
    pub ir_unoptimized: String,
    pub ir_optimized: String,
    pub optimizations_applied: Vec<String>,
    pub assembly: String,
    pub assembly_annotated: Vec<AnnotatedInstruction>,
    pub binary: Vec<u8>,
    pub binary_decoded: Vec<DecodedInstruction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub token_type: String,      // keyword, identifier, literal, operator, punctuation
    pub value: String,            // The actual text
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub explanation: String,      // Beginner-friendly explanation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ASTRepresentation {
    pub root: ASTNode,
    pub visualization_data: String,  // JSON or DOT format for rendering
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ASTNode {
    pub node_type: String,        // FunctionDecl, CompoundStmt, etc.
    pub name: Option<String>,     // For named entities
    pub value: Option<String>,    // For literals
    pub opcode: Option<String>,   // For operators: +, -, *, /, etc.
    pub source_range: SourceRange,
    pub children: Vec<ASTNode>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolTableInfo {
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    pub name: String,             // "global", "main", etc.
    pub parent: Option<String>,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub symbol_type: SymbolType,
    pub data_type: String,        // int, char*, function signature
    pub line_declared: usize,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolType {
    Variable,
    Function,
    Parameter,
    Typedef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotatedInstruction {
    pub address: u64,
    pub bytes: Vec<u8>,
    pub mnemonic: String,
    pub operands: String,
    pub full_instruction: String,
    pub explanation_simple: String,     // Beginner level
    pub explanation_detailed: String,   // Advanced level
    pub affects_flags: Vec<String>,     // ZF, SF, CF, OF
    pub reads_registers: Vec<String>,
    pub writes_registers: Vec<String>,
    pub memory_access: Option<MemoryAccess>,
    pub line_number: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAccess {
    pub access_type: String,  // "read" or "write"
    pub addressing_mode: String,
    pub base_register: Option<String>,
    pub index_register: Option<String>,
    pub scale: Option<u8>,
    pub displacement: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedInstruction {
    pub offset: usize,
    pub bytes: Vec<u8>,
    pub encoding: InstructionEncoding,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionEncoding {
    pub prefixes: Vec<Prefix>,
    pub opcode: Vec<u8>,
    pub modrm: Option<ModRMByte>,
    pub sib: Option<SIBByte>,
    pub displacement: Option<i32>,
    pub immediate: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefix {
    pub byte: u8,
    pub name: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModRMByte {
    pub byte: u8,
    pub mode: u8,       // 2 bits
    pub reg: u8,        // 3 bits
    pub rm: u8,         // 3 bits
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SIBByte {
    pub byte: u8,
    pub scale: u8,      // 2 bits
    pub index: u8,      // 3 bits
    pub base: u8,       // 3 bits
    pub explanation: String,
}
