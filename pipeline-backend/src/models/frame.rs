use serde::{Serialize, Deserialize};

/// Represents a single execution state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionFrame {
    pub index: usize,
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub base_pointer: u64,

    pub registers: Vec<(String, u64)>,

    pub stack: Vec<StackFrame>,
    pub memory_events: Vec<MemoryEvent>,

    pub instruction: String,
    pub opcode_bytes: Vec<u8>,
    pub binary_mapping: BinaryMapping,

    pub stage: ExecutionStage,
    pub explanation_context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    pub return_address: u64,
    pub base_pointer: u64,
    pub locals: Vec<LocalVariable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalVariable {
    pub name: String,
    pub address: u64,
    pub value: u64,
    pub size: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub address: u64,
    pub value: u64,
    pub size: u8,
    pub is_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryMapping {
    pub address: u64,
    pub offset: usize,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExecutionStage {
    Source { line: usize },
    Assembly { instruction: String },
    Execution,
    Binary { bytes: Vec<u8> },
}
