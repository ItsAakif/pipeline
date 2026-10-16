use anyhow::Result;
use iced_x86::{Decoder, Instruction, DecoderOptions, Mnemonic};
use crate::models::{ExecutionFrame, ExecutionStage, StackFrame, MemoryEvent, BinaryMapping, LocalVariable};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
struct CpuFlags {
    zero: bool,      // ZF: Zero flag
    sign: bool,      // SF: Sign flag
    carry: bool,     // CF: Carry flag
    overflow: bool,  // OF: Overflow flag
}

pub struct ExecutionRunner {
    // Will hold instrumentation logic
}

impl ExecutionRunner {
    pub fn new() -> Self {
        Self {}
    }

    /// Run binary and generate execution frames
    /// For v1, this uses static analysis of the binary + simulation
    /// Later phases will add instrumented execution
    pub async fn run(&self, binary: &[u8]) -> Result<Vec<ExecutionFrame>> {
        log::info!("Starting execution trace generation");

        // Hybrid approach: Static analysis + simulation
        let mut frames = Vec::new();

        // Decode binary and simulate execution
        let mut decoder = Decoder::with_ip(64, binary, 0, DecoderOptions::NONE);

        let mut registers = self.initial_registers();
        let mut sp: u64 = 0x7ffff000; // Start stack pointer
        let mut bp: u64 = 0;
        let mut stack: Vec<StackFrame> = Vec::new();
        let mut flags = CpuFlags::default();

        // Simulate up to first 1000 instructions (safety limit)
        let max_instructions = 1000;
        let mut instruction_count = 0;

        // A single decoder advances through `binary`. Re-creating it with
        // `Decoder::with_ip` on every iteration only changes the reported IP
        // (it does not seek), which previously re-decoded byte 0 forever.
        while decoder.can_decode() {
            if instruction_count >= max_instructions {
                log::warn!("Reached instruction limit, stopping execution");
                break;
            }

            let instruction = decoder.decode();
            let ip = instruction.ip();

            // Simulate instruction execution and track memory events
            let mut memory_events = Vec::new();
            self.simulate_instruction(&instruction, &mut registers, &mut sp, &mut bp, &mut stack, &mut memory_events, &mut flags)?;

            // Generate frame with memory events
            let frame = self.create_frame(
                instruction_count,
                ip,
                sp,
                bp,
                &registers,
                &stack,
                &instruction,
                binary,
                ExecutionStage::Execution,
                memory_events,
            )?;

            frames.push(frame);

            instruction_count += 1;
        }

        log::info!("Generated {} execution frames", frames.len());
        Ok(frames)
    }

    fn initial_registers(&self) -> HashMap<String, u64> {
        let mut regs = HashMap::new();
        regs.insert("rax".to_string(), 0);
        regs.insert("rbx".to_string(), 0);
        regs.insert("rcx".to_string(), 0);
        regs.insert("rdx".to_string(), 0);
        regs.insert("rsi".to_string(), 0);
        regs.insert("rdi".to_string(), 0);
        regs.insert("r8".to_string(), 0);
        regs.insert("r9".to_string(), 0);
        regs.insert("r10".to_string(), 0);
        regs.insert("r11".to_string(), 0);
        regs.insert("r12".to_string(), 0);
        regs.insert("r13".to_string(), 0);
        regs.insert("r14".to_string(), 0);
        regs.insert("r15".to_string(), 0);
        regs
    }

    fn create_frame(
        &self,
        index: usize,
        ip: u64,
        sp: u64,
        bp: u64,
        registers: &HashMap<String, u64>,
        stack: &[StackFrame],
        instruction: &Instruction,
        binary: &[u8],
        stage: ExecutionStage,
        memory_events: Vec<MemoryEvent>,
    ) -> Result<ExecutionFrame> {
        let opcode_start = ip as usize;
        let opcode_end = (ip + instruction.len() as u64) as usize;
        let opcode_bytes = if opcode_end <= binary.len() {
            binary[opcode_start..opcode_end].to_vec()
        } else {
            vec![]
        };

        let reg_vec = registers.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        let explanation = self.generate_explanation(instruction);

        Ok(ExecutionFrame {
            index,
            instruction_pointer: ip,
            stack_pointer: sp,
            base_pointer: bp,
            registers: reg_vec,
            stack: stack.to_vec(),
            memory_events,
            instruction: instruction.to_string(),
            opcode_bytes,
            binary_mapping: BinaryMapping {
                address: ip,
                offset: opcode_start,
                size: instruction.len(),
            },
            stage,
            explanation_context: explanation,
        })
    }

    fn generate_explanation(&self, instruction: &Instruction) -> String {
        match instruction.mnemonic() {
            Mnemonic::Mov => {
                format!(
                    "MOV: Copy value from source to destination. This is a data transfer operation."
                )
            }
            Mnemonic::Push => {
                format!(
                    "PUSH: Save value to stack. Decrements stack pointer by 8 bytes and stores the value at the new stack top."
                )
            }
            Mnemonic::Pop => {
                format!(
                    "POP: Restore value from stack. Loads value from current stack top and increments stack pointer by 8 bytes."
                )
            }
            Mnemonic::Call => {
                format!(
                    "CALL: Function call. Pushes return address onto stack and transfers control to the called function."
                )
            }
            Mnemonic::Ret => {
                "RET: Return from function. Pops return address from stack and transfers control back to caller.".to_string()
            }
            Mnemonic::Add => {
                format!(
                    "ADD: Addition operation. Adds source to destination and stores result. Updates CPU flags (ZF, SF, CF, OF)."
                )
            }
            Mnemonic::Sub => {
                format!(
                    "SUB: Subtraction operation. Subtracts source from destination and stores result. Updates CPU flags (ZF, SF, CF, OF)."
                )
            }
            Mnemonic::Cmp => {
                "CMP: Compare values. Performs subtraction to set flags but doesn't store result. Used before conditional jumps.".to_string()
            }
            Mnemonic::Test => {
                "TEST: Logical test. Performs bitwise AND to set flags but doesn't store result. Often used to check if value is zero.".to_string()
            }
            Mnemonic::Je => {
                "JE: Jump if equal/zero. Branches if Zero Flag (ZF) is set. Used after CMP or TEST.".to_string()
            }
            Mnemonic::Jne => {
                "JNE: Jump if not equal/zero. Branches if Zero Flag (ZF) is clear. Used after CMP or TEST.".to_string()
            }
            Mnemonic::Jg => {
                "JG: Jump if greater (signed). Branches if ZF=0 and SF=OF. Used after CMP for signed comparison.".to_string()
            }
            Mnemonic::Jl => {
                "JL: Jump if less (signed). Branches if SF≠OF. Used after CMP for signed comparison.".to_string()
            }
            Mnemonic::Jmp => {
                "JMP: Unconditional jump. Always transfers control to target address.".to_string()
            }
            Mnemonic::Lea => {
                "LEA: Load Effective Address. Calculates memory address but doesn't access memory. Often used for pointer arithmetic.".to_string()
            }
            Mnemonic::Inc => {
                "INC: Increment by 1. Adds 1 to the operand and updates flags (except CF).".to_string()
            }
            Mnemonic::Dec => {
                "DEC: Decrement by 1. Subtracts 1 from the operand and updates flags (except CF).".to_string()
            }
            Mnemonic::Mul | Mnemonic::Imul => {
                "MUL/IMUL: Multiply. Performs multiplication (unsigned/signed) and stores result in RAX.".to_string()
            }
            Mnemonic::Div | Mnemonic::Idiv => {
                "DIV/IDIV: Divide. Performs division (unsigned/signed) with quotient in RAX and remainder in RDX.".to_string()
            }
            Mnemonic::And => {
                "AND: Bitwise AND. Performs logical AND operation between operands. Clears OF and CF, updates ZF and SF.".to_string()
            }
            Mnemonic::Or => {
                "OR: Bitwise OR. Performs logical OR operation between operands. Clears OF and CF, updates ZF and SF.".to_string()
            }
            Mnemonic::Xor => {
                "XOR: Bitwise XOR. Performs exclusive OR operation. Clears OF and CF, updates ZF and SF. XOR reg, reg sets reg to zero.".to_string()
            }
            Mnemonic::Not => {
                "NOT: Bitwise NOT. Inverts all bits of the operand (one's complement). Does not affect flags.".to_string()
            }
            Mnemonic::Shl | Mnemonic::Sal => {
                "SHL/SAL: Shift left. Shifts bits left, filling with zeros. Updates CF (last bit shifted out), ZF, SF, OF.".to_string()
            }
            Mnemonic::Shr => {
                "SHR: Logical shift right. Shifts bits right, filling with zeros. Updates CF (last bit shifted out), ZF, SF.".to_string()
            }
            Mnemonic::Sar => {
                "SAR: Arithmetic shift right. Shifts bits right, preserving sign bit. Updates CF, ZF, SF, OF.".to_string()
            }
            Mnemonic::Nop => {
                "NOP: No operation. Does nothing. Often used for alignment or placeholder.".to_string()
            }
            _ => format!("{:?}: Instruction executed", instruction.mnemonic()),
        }
    }

    /// Simplified instruction simulation for educational visualization
    /// This is NOT a full CPU emulator, just enough for visualization
    fn simulate_instruction(
        &self,
        instruction: &Instruction,
        registers: &mut HashMap<String, u64>,
        sp: &mut u64,
        bp: &mut u64,
        stack: &mut Vec<StackFrame>,
        memory_events: &mut Vec<MemoryEvent>,
        flags: &mut CpuFlags,
    ) -> Result<()> {
        match instruction.mnemonic() {
            Mnemonic::Push => {
                // Get value to push before decrementing SP
                let value = self.extract_operand_value(instruction, 0, registers)?;

                // Decrement stack pointer
                *sp -= 8;

                // Record memory write event
                memory_events.push(MemoryEvent {
                    address: *sp,
                    value,
                    size: 8,
                    is_write: true,
                });
            }
            Mnemonic::Pop => {
                // Record memory read event before incrementing SP
                memory_events.push(MemoryEvent {
                    address: *sp,
                    value: 0, // Value not tracked in stack simulation
                    size: 8,
                    is_write: false,
                });

                // Increment stack pointer
                *sp += 8;
            }
            Mnemonic::Call => {
                // Calculate return address (next instruction)
                let return_address = instruction.len() as u64;

                // Decrement stack pointer
                *sp -= 8;

                // Record return address write to stack
                memory_events.push(MemoryEvent {
                    address: *sp,
                    value: return_address,
                    size: 8,
                    is_write: true,
                });

                // Create new stack frame
                stack.push(StackFrame {
                    return_address: *sp + 8, // Simplified
                    base_pointer: *bp,
                    locals: self.extract_local_variables(*sp, *bp),
                });

                *bp = *sp;
            }
            Mnemonic::Ret => {
                // Record memory read event for return address
                if !stack.is_empty() {
                    memory_events.push(MemoryEvent {
                        address: *sp,
                        value: 0, // Return address not tracked in simulation
                        size: 8,
                        is_write: false,
                    });
                }

                // Pop stack frame
                if let Some(frame) = stack.pop() {
                    *bp = frame.base_pointer;
                    *sp += 8;
                }
            }
            Mnemonic::Mov => {
                // Extract source value
                let src_value = self.extract_operand_value(instruction, 1, registers)?;

                // Set destination operand
                self.set_operand_value(instruction, 0, registers, src_value)?;
            }
            Mnemonic::Add => {
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest.wrapping_add(src);
                self.set_flags_arithmetic(dest, src, result, flags, false);
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Sub => {
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest.wrapping_sub(src);
                self.set_flags_arithmetic(dest, src, result, flags, true);
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Inc => {
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest.wrapping_add(1);
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Dec => {
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest.wrapping_sub(1);
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Mul | Mnemonic::Imul => {
                // MUL/IMUL: RAX * src -> RDX:RAX (simplified, just RAX for now)
                if instruction.op_count() > 0 {
                    let src = self.extract_operand_value(instruction, 0, registers)?;
                    let rax = *registers.get("rax").unwrap_or(&0);
                    let result = rax.wrapping_mul(src);
                    registers.insert("rax".to_string(), result);
                }
            }
            Mnemonic::Div | Mnemonic::Idiv => {
                // DIV/IDIV: RDX:RAX / src (simplified, just RAX for now)
                if instruction.op_count() > 0 {
                    let src = self.extract_operand_value(instruction, 0, registers)?;
                    let rax = *registers.get("rax").unwrap_or(&0);
                    if src != 0 {
                        let result = rax / src;
                        registers.insert("rax".to_string(), result);
                    }
                }
            }
            Mnemonic::And => {
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest & src;
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Or => {
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest | src;
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Xor => {
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest ^ src;
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Not => {
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = !dest;
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Cmp => {
                // CMP: Subtract and set flags but don't store result
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest.wrapping_sub(src);
                self.set_flags_arithmetic(dest, src, result, flags, true);
            }
            Mnemonic::Test => {
                // TEST: Bitwise AND and set flags but don't store result
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest & src;
                flags.zero = result == 0;
                flags.sign = (result as i64) < 0;
                flags.carry = false;
                flags.overflow = false;
            }
            Mnemonic::Shl | Mnemonic::Sal => {
                // SHL/SAL: Shift left
                let count = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = dest.wrapping_shl(count as u32);
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Shr | Mnemonic::Sar => {
                // SHR/SAR: Shift right
                let count = self.extract_operand_value(instruction, 1, registers)?;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = if instruction.mnemonic() == Mnemonic::Shr {
                    dest.wrapping_shr(count as u32)
                } else {
                    // SAR: arithmetic shift right (preserve sign)
                    ((dest as i64).wrapping_shr(count as u32)) as u64
                };
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Jmp => {
                // JMP: Unconditional jump (would update IP in real execution)
                // For visualization, we just record it happened
            }
            Mnemonic::Je => {
                // JE: Jump if equal/zero (ZF=1)
                if flags.zero {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jne => {
                // JNE: Jump if not equal/zero (ZF=0)
                if !flags.zero {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jg => {
                // JG: Jump if greater (ZF=0 and SF=OF)
                if !flags.zero && (flags.sign == flags.overflow) {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jge => {
                // JGE: Jump if greater or equal (SF=OF)
                if flags.sign == flags.overflow {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jl => {
                // JL: Jump if less (SF!=OF)
                if flags.sign != flags.overflow {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jle => {
                // JLE: Jump if less or equal (ZF=1 or SF!=OF)
                if flags.zero || (flags.sign != flags.overflow) {
                    // Would jump in real execution
                }
            }
            Mnemonic::Ja => {
                // JA: Jump if above (CF=0 and ZF=0)
                if !flags.carry && !flags.zero {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jae => {
                // JAE: Jump if above or equal (CF=0)
                if !flags.carry {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jb => {
                // JB: Jump if below (CF=1)
                if flags.carry {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jbe => {
                // JBE: Jump if below or equal (CF=1 or ZF=1)
                if flags.carry || flags.zero {
                    // Would jump in real execution
                }
            }
            Mnemonic::Js => {
                // JS: Jump if sign (SF=1)
                if flags.sign {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jns => {
                // JNS: Jump if not sign (SF=0)
                if !flags.sign {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jo => {
                // JO: Jump if overflow (OF=1)
                if flags.overflow {
                    // Would jump in real execution
                }
            }
            Mnemonic::Jno => {
                // JNO: Jump if not overflow (OF=0)
                if !flags.overflow {
                    // Would jump in real execution
                }
            }
            Mnemonic::Lea => {
                // LEA: Load effective address (calculates address but doesn't access memory)
                // Simplified for visualization
                let value = self.extract_operand_value(instruction, 1, registers)?;
                self.set_operand_value(instruction, 0, registers, value)?;
            }
            Mnemonic::Nop => {
                // NOP: No operation
            }
            Mnemonic::Leave => {
                // LEAVE: Set SP to BP, then pop BP
                *sp = *bp;
                if !stack.is_empty() {
                    memory_events.push(MemoryEvent {
                        address: *sp,
                        value: 0,
                        size: 8,
                        is_write: false,
                    });
                    if let Some(frame) = stack.pop() {
                        *bp = frame.base_pointer;
                    }
                    *sp += 8;
                }
            }
            Mnemonic::Enter => {
                // ENTER: Create stack frame
                *sp -= 8;
                memory_events.push(MemoryEvent {
                    address: *sp,
                    value: *bp,
                    size: 8,
                    is_write: true,
                });
                let locals = self.extract_local_variables(*sp, *bp);
                stack.push(StackFrame {
                    return_address: *sp + 8,
                    base_pointer: *bp,
                    locals,
                });
                *bp = *sp;
            }
            Mnemonic::Neg => {
                // NEG: Two's complement negation
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = (-(dest as i64)) as u64;
                flags.carry = dest != 0;
                flags.zero = result == 0;
                flags.sign = (result as i64) < 0;
                flags.overflow = dest == 0x8000000000000000;
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Movsx | Mnemonic::Movsxd => {
                // MOVSX: Move with sign extension
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let result = (src as i32) as i64 as u64; // Sign extend from 32-bit
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Movzx => {
                // MOVZX: Move with zero extension
                let src = self.extract_operand_value(instruction, 1, registers)?;
                let result = src & 0xFFFFFFFF; // Zero extend from 32-bit
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Rol | Mnemonic::Ror => {
                // ROL/ROR: Rotate left/right
                let count = self.extract_operand_value(instruction, 1, registers)? as u32;
                let dest = self.extract_operand_value(instruction, 0, registers)?;
                let result = if instruction.mnemonic() == Mnemonic::Rol {
                    dest.rotate_left(count)
                } else {
                    dest.rotate_right(count)
                };
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            Mnemonic::Setb | Mnemonic::Sete | Mnemonic::Setne | Mnemonic::Setg | 
            Mnemonic::Setge | Mnemonic::Setl | Mnemonic::Setle => {
                // SETcc: Set byte based on condition flags
                let condition = match instruction.mnemonic() {
                    Mnemonic::Setb => flags.carry,
                    Mnemonic::Sete => flags.zero,
                    Mnemonic::Setne => !flags.zero,
                    Mnemonic::Setg => !flags.zero && (flags.sign == flags.overflow),
                    Mnemonic::Setge => flags.sign == flags.overflow,
                    Mnemonic::Setl => flags.sign != flags.overflow,
                    Mnemonic::Setle => flags.zero || (flags.sign != flags.overflow),
                    _ => false,
                };
                let result = if condition { 1 } else { 0 };
                self.set_operand_value(instruction, 0, registers, result)?;
            }
            _ => {
                // Other instructions - no simulation needed for visualization
            }
        }

        Ok(())
    }

    /// Set CPU flags based on arithmetic operation result
    fn set_flags_arithmetic(&self, dest: u64, src: u64, result: u64, flags: &mut CpuFlags, is_sub: bool) {
        flags.zero = result == 0;
        flags.sign = (result as i64) < 0;
        
        if is_sub {
            // Carry flag for subtraction: set if borrow occurred
            flags.carry = dest < src;
            // Overflow for subtraction: sign changed unexpectedly
            let dest_sign = (dest as i64) < 0;
            let src_sign = (src as i64) < 0;
            let result_sign = (result as i64) < 0;
            flags.overflow = (dest_sign != src_sign) && (dest_sign != result_sign);
        } else {
            // Carry flag for addition: set if unsigned overflow
            flags.carry = result < dest || result < src;
            // Overflow for addition: sign changed unexpectedly
            let dest_sign = (dest as i64) < 0;
            let src_sign = (src as i64) < 0;
            let result_sign = (result as i64) < 0;
            flags.overflow = (dest_sign == src_sign) && (dest_sign != result_sign);
        }
    }

    fn extract_operand_value(
        &self,
        instruction: &Instruction,
        operand_index: u32,
        registers: &HashMap<String, u64>,
    ) -> Result<u64> {
        use iced_x86::OpKind;

        if operand_index >= instruction.op_count() {
            return Ok(0);
        }

        match instruction.op_kind(operand_index) {
            OpKind::Register => {
                let reg = instruction.op_register(operand_index);
                // Register keys are normalized to lowercase (see `set_operand_value`
                // and `initial_registers`), so reads must normalize too.
                Ok(registers
                    .get(&format!("{:?}", reg).to_lowercase())
                    .copied()
                    .unwrap_or(0))
            }
            OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate64
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
            | OpKind::Immediate8to64
            | OpKind::Immediate32to64 => Ok(instruction.immediate(operand_index) as u64),
            _ => Ok(0),
        }
    }

    /// Set the value of a destination operand (register or memory)
    /// For educational visualization, we only handle register operands
    fn set_operand_value(
        &self,
        instruction: &Instruction,
        operand_index: u32,
        registers: &mut HashMap<String, u64>,
        value: u64,
    ) -> Result<()> {
        use iced_x86::OpKind;

        if operand_index >= instruction.op_count() {
            return Ok(());
        }

        match instruction.op_kind(operand_index) {
            OpKind::Register => {
                let reg = instruction.op_register(operand_index);
                let reg_name = format!("{:?}", reg).to_lowercase();
                registers.insert(reg_name, value);
            }
            OpKind::Memory => {
                // Memory operand - would need to calculate effective address
                // and record in memory_events
                // For now, this is simplified and not fully implemented
                log::debug!("Memory operand write not fully implemented: addr unknown");
            }
            _ => {
                // Other operand types not supported for writes
            }
        }

        Ok(())
    }

    /// Extract local variables from stack (simplified simulation)
    fn extract_local_variables(&self, sp: u64, bp: u64) -> Vec<LocalVariable> {
        let mut locals = Vec::new();
        
        // Simulate 1-3 local variables per stack frame
        let frame_size = if bp > sp { bp - sp } else { 0 };
        let var_count = (frame_size / 8).min(3) as usize;
        
        for i in 0..var_count {
            locals.push(LocalVariable {
                name: format!("var{}", i),
                address: sp + (i as u64 * 8),
                value: 0, // Would need to read from simulated memory
                size: 8,
            });
        }
        
        locals
    }
}

impl Default for ExecutionRunner {
    fn default() -> Self {
        Self::new()
    }
}
