// Educational compilation stages
export interface TokenInfo {
  token_type: string;
  value: string;
  line: number;
  column: number;
  length: number;
  explanation: string;
}

export interface ASTNode {
  node_type: string;
  name: string | null;
  value: string | null;
  opcode: string | null;
  source_range: SourceRange;
  children: ASTNode[];
  explanation: string;
}

export interface SourceRange {
  start_line: number;
  start_column: number;
  end_line: number;
  end_column: number;
}

export interface ASTRepresentation {
  root: ASTNode;
  visualization_data: string;
}

export interface SymbolTableInfo {
  scopes: Scope[];
}

export interface Scope {
  name: string;
  parent: string | null;
  symbols: Symbol[];
}

export interface Symbol {
  name: string;
  symbol_type: 'variable' | 'function' | 'parameter' | 'typedef';
  data_type: string;
  line_declared: number;
  explanation: string;
}

export interface AnnotatedInstruction {
  address: number;
  bytes: number[];
  mnemonic: string;
  operands: string;
  full_instruction: string;
  explanation_simple: string;
  explanation_detailed: string;
  affects_flags: string[];
  reads_registers: string[];
  writes_registers: string[];
  memory_access: MemoryAccess | null;
  line_number: number;
}

export interface MemoryAccess {
  access_type: string;
  addressing_mode: string;
  base_register: string | null;
  index_register: string | null;
  scale: number | null;
  displacement: number | null;
}

export interface DecodedInstruction {
  offset: number;
  bytes: number[];
  encoding: InstructionEncoding;
  explanation: string;
}

export interface InstructionEncoding {
  prefixes: Prefix[];
  opcode: number[];
  modrm: ModRMByte | null;
  sib: SIBByte | null;
  displacement: number | null;
  immediate: number | null;
}

export interface Prefix {
  byte: number;
  name: string;
  purpose: string;
}

export interface ModRMByte {
  byte: number;
  mode: number;
  reg: number;
  rm: number;
  explanation: string;
}

export interface SIBByte {
  byte: number;
  scale: number;
  index: number;
  base: number;
  explanation: string;
}

export interface CompilationStages {
  tokens: TokenInfo[];
  ast: ASTRepresentation | null;
  symbols: SymbolTableInfo;
  ir_unoptimized: string;
  ir_optimized: string;
  optimizations_applied: string[];
  assembly: string;
  assembly_annotated: AnnotatedInstruction[];
  binary: number[];
  binary_decoded: DecodedInstruction[];
}

export interface CompileResponse {
  success: boolean;
  assembly?: string;
  source?: string;
  binary?: string;
  stages?: CompilationStages;
  error?: string;
  compilation_time_ms: number;
}

// POST /api/run — compile + link + execute the program and return its output.
export interface RunResponse {
  success: boolean;
  compile_error: string | null;
  stdout: string;
  stderr: string;
  exit_code: number;
  timed_out: boolean;
  truncated: boolean;
  execution_time_ms: number;
  // Present on a 429 cooldown response.
  error?: string;
  retry_after_secs?: number;
}
