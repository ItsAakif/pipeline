# Pipeline Architecture: Educational Compiler Visualization

## Vision
Transform Pipeline into a comprehensive educational tool that visualizes EVERY stage of code compilation from source to binary, with detailed explanations suitable for beginners.

## Compilation Pipeline Stages

### 1. **Lexical Analysis (Tokenization)**
**What:** Break source code into tokens
**How:** Use clang's preprocessor and lexer APIs
**Output:** List of tokens with types, positions, and values
**Visualization:**
- Color-coded tokens
- Token type labels (keyword, identifier, literal, operator, punctuation)
- Position mapping back to source

**Example:**
```c
int main() {
```
Becomes:
- `INT` (keyword) at line 1, col 1-3
- `IDENTIFIER("main")` at line 1, col 5-8
- `LPAREN` (punctuation) at line 1, col 9
- `RPAREN` (punctuation) at line 1, col 10
- `LBRACE` (punctuation) at line 1, col 12

**Explanation:** "The lexer reads the source code character by character and groups them into meaningful tokens. Keywords like 'int' are identified, names like 'main' become identifiers, and symbols like '(' become punctuation tokens."

### 2. **Syntax Analysis (Parsing/AST)**
**What:** Build Abstract Syntax Tree from tokens
**How:** Use clang `-ast-dump=json` to get structured AST
**Output:** Tree structure showing program organization
**Visualization:**
- Interactive tree diagram
- Node types (FunctionDecl, CompoundStmt, ReturnStmt, etc.)
- Parent-child relationships
- Source location mapping

**Example AST:**
```
TranslationUnitDecl
  FunctionDecl (main)
    ParmVarDecl (void parameters)
    CompoundStmt (function body)
      ReturnStmt
        IntegerLiteral (0)
```

**Explanation:** "The parser organizes tokens into a tree structure that represents the program's grammatical structure. Each function becomes a FunctionDecl node, statements become children, expressions become leaves."

### 3. **Semantic Analysis**
**What:** Type checking, symbol resolution, semantic validation
**How:** Extract from clang's Sema phase
**Output:** Symbol table, type information, scope analysis
**Visualization:**
- Symbol table showing all variables/functions with types
- Scope hierarchy
- Type checking decisions
- Error locations if any

**Example Symbol Table:**
```
Global Scope:
  - main: function(void) -> int

main() scope:
  - (no local variables)
```

**Explanation:** "The semantic analyzer checks if your code makes sense. It verifies that functions are called with correct arguments, variables are declared before use, and types match (you can't add a string to an integer)."

### 4. **LLVM IR Generation**
**What:** Convert AST to LLVM Intermediate Representation
**How:** Use clang `-emit-llvm -S` to generate human-readable LLVM IR
**Output:** LLVM IR code
**Visualization:**
- Side-by-side: C code <-> LLVM IR
- Highlighted correspondence
- IR instruction explanations

**Example:**
```llvm
define i32 @main() {
entry:
  ret i32 0
}
```

**Explanation:** "LLVM IR is a low-level programming language that's easier for the compiler to optimize than C. It's platform-independent - the same IR works for Intel, ARM, or any processor. Think of it as a 'universal assembly language'."

### 5. **Optimization Passes**
**What:** Apply transformations to improve code
**How:** Run clang with `-O2` and without, compare IR
**Output:** Before/after IR for each optimization pass
**Visualization:**
- Side-by-side before/after comparison
- Highlight changed sections
- Explain what each optimization does

**Common Optimizations:**
- Dead code elimination
- Constant folding
- Inline expansion
- Loop unrolling
- Register allocation hints

**Explanation:** "The optimizer makes your code faster and smaller without changing what it does. For example, if you write 'x = 5 + 3', the optimizer calculates '8' at compile-time instead of at runtime."

### 6. **Assembly Generation (Code Generation)**
**What:** Convert LLVM IR to target assembly (x86-64)
**How:** Use clang `-S -masm=intel` for Intel syntax assembly
**Output:** Assembly instructions with detailed annotations
**Visualization:**
- Assembly instructions with color coding
- Instruction format breakdown
- Register usage tracking
- Calling convention explanation

**Example:**
```asm
main:
    push rbp           ; Save base pointer
    mov rbp, rsp       ; Set up stack frame
    xor eax, eax       ; Set return value to 0
    pop rbp            ; Restore base pointer
    ret                ; Return to caller
```

**Explanation:** "Assembly is human-readable machine code. Each instruction tells the CPU exactly what to do: 'mov' moves data, 'xor' does bitwise XOR (used here as a trick to set eax to 0), 'ret' returns from the function."

### 7. **Binary Encoding (Machine Code)**
**What:** Convert assembly to actual machine code bytes
**How:** Use objdump to disassemble and show encoding
**Output:** Hex bytes with instruction decoding
**Visualization:**
- Byte-by-byte breakdown
- Opcode identification
- ModR/M and SIB byte decoding
- Immediate values and offsets

**Example:**
```
55                    push rbp
48 89 e5              mov rbp, rsp
31 c0                 xor eax, eax
5d                    pop rbp
c3                    ret
```

**Breakdown:**
- `55`: Opcode for PUSH RBP
- `48`: REX.W prefix (64-bit operand size)
- `89`: Opcode for MOV r/m64, r64
- `e5`: ModR/M byte (11 100 101 = direct addressing, RSP -> RBP)
- `31`: Opcode for XOR r/m32, r32
- `c0`: ModR/M byte (11 000 000 = EAX XOR EAX)
- `5d`: Opcode for POP RBP
- `c3`: Opcode for RET

**Explanation:** "This is what the CPU actually executes - bytes in memory. Each instruction has an opcode (operation code) that tells the CPU what operation to perform. Additional bytes specify which registers or memory addresses to use."

### 8. **Program Output (real execution)**
**What:** Compile, link and run the program, then show its real output
**How:** `POST /api/run` builds a native executable (`clang -O0 -o prog source.c`)
and runs it under a sandbox (wall-clock timeout, `ulimit` CPU/memory/process/
file-size limits, output caps, non-root user)
**Output:** The program's real `stdout`, `stderr` and exit code
**Visualization:**
- stdout in a monospace block; stderr shown distinctly
- The exit code, plus "timed out" / "truncated" notices when applicable
- A clear message when the program failed to compile

**Explanation:** "This is not a simulation - the program is compiled, linked and
actually run inside the container, so you see exactly what it prints and the code
it exits with. A separate legacy endpoint, `POST /api/execute`, still returns a
simulated instruction timeline, but the UI no longer uses it."

## Backend Implementation Plan

### New Compilation Orchestrator
```rust
pub struct CompilationPipeline {
    // Captures all stages
    pub tokens: Vec<Token>,
    pub ast: ASTNode,
    pub symbol_table: SymbolTable,
    pub llvm_ir: String,
    pub llvm_ir_optimized: String,
    pub assembly: String,
    pub binary: Vec<u8>,
    pub disassembly: String,
}
```

### Clang Integration Strategy
1. **Tokenization:** Run clang with custom plugin or use `clang -Xclang -dump-tokens`
2. **AST:** Use `clang -Xclang -ast-dump=json`
3. **LLVM IR (unoptimized):** Use `clang -S -emit-llvm -O0`
4. **LLVM IR (optimized):** Use `clang -S -emit-llvm -O2`
5. **Assembly:** Use `clang -S -masm=intel -O2`
6. **Binary:** Use `clang -c` (already done)
7. **Disassembly:** Use `objdump -d -Mintel` (already done)

### New Response Format
```rust
pub struct CompileResponse {
    pub success: bool,
    pub stages: CompilationStages,
    pub explanations: StageExplanations,
    pub error: Option<String>,
}

pub struct CompilationStages {
    pub tokens: Vec<TokenInfo>,
    pub ast: ASTRepresentation,
    pub symbols: SymbolTableInfo,
    pub ir_unoptimized: String,
    pub ir_optimized: String,
    pub optimizations_applied: Vec<OptimizationPass>,
    pub assembly: AnnotatedAssembly,
    pub binary: BinaryRepresentation,
    pub binary_decoded: Vec<DecodedInstruction>,
}
```

## Frontend Implementation Plan

### New Stage Navigation
```
[ Tokens ] [ AST ] [ Symbols ] [ IR ] [ Opt ] [ Asm ] [ Bin ] [ Output ]
```

### Visualization Components
1. **TokenView:** Color-coded token list with type labels
2. **ASTView:** Interactive tree diagram with node details
3. **SymbolView:** Table showing all symbols with scopes
4. **IRView:** Side-by-side comparison with source mapping
5. **OptimizationView:** Diff view showing transformations
6. **AssemblyView:** Annotated assembly with explanations
7. **BinaryView:** Hex dump with instruction decoding
8. **OutputView:** Compiles, links and runs the program (`POST /api/run`) and shows its real stdout/stderr/exit code

### Educational Content System
```typescript
interface Explanation {
  level: 'beginner' | 'intermediate' | 'advanced';
  stage: CompilationStage;
  concept: string;
  text: string;
  examples?: string[];
  visualAids?: string[];
}
```

## Technical Challenges

### 1. Clang Integration
- Need to run clang multiple times with different flags
- Parse various output formats (JSON AST, LLVM IR text, assembly)
- Error handling when compilation fails at any stage

### 2. AST Visualization
- JSON AST can be huge - need smart filtering/collapsing
- Interactive tree requires SVG or Canvas rendering
- Performance with large files

### 3. LLVM IR Parsing
- Need to parse and annotate LLVM IR
- Map IR instructions back to source lines
- Explain IR concepts (SSA, basic blocks, phi nodes)

### 4. Binary Decoding
- Parse x86-64 instruction encoding
- Decode ModR/M, SIB, displacement, immediate bytes
- Handle variable-length instructions

### 5. Performance
- Running clang 5-6 times per compilation is slow
- Need caching strategy
- Consider parallel execution where possible

### 6. Educational Content
- Write hundreds of explanations
- Multiple difficulty levels
- Keep explanations accurate yet accessible

## Implementation Phases

### Phase 1: Infrastructure (Week 1)
- [ ] Update backend models for all stages
- [ ] Implement multi-stage clang pipeline
- [ ] Parse clang outputs (tokens, AST JSON, IR, asm)
- [ ] Update API to return all stages

### Phase 2: Token & AST (Week 2)
- [ ] Implement tokenization extraction
- [ ] Parse AST JSON into usable format
- [ ] Create TokenView frontend component
- [ ] Create ASTView with interactive tree
- [ ] Write beginner explanations for tokens/AST

### Phase 3: Semantic & IR (Week 3)
- [ ] Extract symbol table from AST
- [ ] Capture both unoptimized and optimized IR
- [ ] Create SymbolView component
- [ ] Create IRView with diff capability
- [ ] Write IR concept explanations

### Phase 4: Optimization (Week 4)
- [ ] Identify optimization passes applied
- [ ] Create before/after comparisons
- [ ] Build OptimizationView component
- [ ] Explain common optimizations

### Phase 5: Assembly & Binary (Week 5)
- [ ] Annotate assembly with detailed comments
- [ ] Implement x86-64 instruction decoder
- [ ] Create enhanced AssemblyView
- [ ] Create detailed BinaryView with byte breakdown
- [ ] Explain instruction encoding

### Phase 6: Polish & Content (Week 6)
- [ ] Add the Output view (real program execution via `/api/run`)
- [ ] Add beginner/intermediate/advanced toggle
- [ ] Write comprehensive explanations for all stages
- [ ] Add interactive tutorials
- [ ] Performance optimization
- [ ] Documentation

## Success Metrics

A user should be able to:
1. Input simple C code
2. See it tokenized with token types explained
3. View the AST and understand program structure
4. See symbol tables and understand scoping
5. Read LLVM IR and understand basic concepts
6. See what optimizations were applied and why
7. Read assembly and understand each instruction
8. See binary encoding and understand opcodes
9. Run the program and see its real stdout, stderr and exit code
10. Toggle between beginner and advanced explanations

## Resources Needed

- Clang documentation for flags and output formats
- LLVM IR language reference
- x86-64 instruction set reference (Intel/AMD manuals)
- Research on educational compiler tools (Compiler Explorer, Wandbox, etc.)
- UI/UX research for complex data visualization

## Estimated Timeline

**6 weeks of focused development** with one person working full-time, or **12 weeks** part-time.

This is a significant undertaking but will create something truly valuable for learning how compilers work.
