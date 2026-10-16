// A small curated map of common x86-64 mnemonics to plain-English descriptions.
// Used for hover tooltips on assembly instructions.

export const OPCODE_DESCRIPTIONS: Record<string, string> = {
  // Data movement
  mov: 'Copy a value from a source into a destination (register or memory).',
  movq: 'Move 64 bits (quad word) between registers/memory.',
  movl: 'Move 32 bits (long word) between registers/memory.',
  lea: 'Load the effective address of a memory operand into a register.',
  push: 'Push a value onto the stack and decrement the stack pointer.',
  pop: 'Pop a value off the stack into a register and increment the stack pointer.',
  xchg: 'Exchange the values of two operands.',
  movzx: 'Move a smaller value into a larger register, filling the top bits with zeros.',
  movsx: 'Move a smaller value into a larger register, sign-extending the top bits.',

  // Arithmetic
  add: 'Add two operands; the result is stored in the destination.',
  sub: 'Subtract the source from the destination.',
  imul: 'Signed integer multiply.',
  mul: 'Unsigned integer multiply.',
  idiv: 'Signed integer divide; the quotient and remainder land in registers.',
  div: 'Unsigned integer divide.',
  inc: 'Increment the operand by one.',
  dec: 'Decrement the operand by one.',
  neg: 'Negate (two\'s complement) the operand.',
  cdqe: 'Sign-extend EAX into RAX before a wider arithmetic operation.',

  // Logic / bit manipulation
  and: 'Bitwise AND of two operands.',
  or: 'Bitwise OR of two operands.',
  xor: 'Bitwise exclusive OR; XOR a register with itself to zero it.',
  not: 'Bitwise complement (flip every bit).',
  shl: 'Shift the operand left by the given number of bits.',
  shr: 'Logical shift the operand right.',
  sar: 'Arithmetic shift the operand right, preserving the sign bit.',
  rol: 'Rotate the operand left.',
  ror: 'Rotate the operand right.',

  // Comparison / control flow
  cmp: 'Compare two operands by subtracting them and setting flags (no result stored).',
  test: 'Bitwise AND two operands to set flags without storing a result.',
  je: 'Jump if equal (zero flag set).',
  jne: 'Jump if not equal (zero flag clear).',
  jz: 'Jump if zero (alias of JE).',
  jnz: 'Jump if not zero (alias of JNE).',
  jg: 'Jump if greater (signed).',
  jge: 'Jump if greater or equal (signed).',
  jl: 'Jump if less (signed).',
  jle: 'Jump if less or equal (signed).',
  ja: 'Jump if above (unsigned).',
  jb: 'Jump if below (unsigned).',
  jmp: 'Unconditional jump to a target address.',
  call: 'Call a function: push the return address and jump to the target.',
  ret: 'Return from a function: pop the return address and jump back.',
  leave: 'Restore the caller\'s stack frame (mov rsp, rbp; pop rbp).',
  nop: 'No operation; wastes a cycle to align or pad code.',

  // Comparisons / conversions
  seta: 'Set a byte to 1 if the "above" condition holds.',
  sete: 'Set a byte to 1 if equal.',
  setne: 'Set a byte to 1 if not equal.',
  cqo: 'Sign-extend RAX into RDX:RAX before a wider divide.',
  cltq: 'Sign-extend EAX into RAX (alias of CDQE).',

  // Floating point / SSE
  addsd: 'Add two scalar double-precision floats.',
  subsd: 'Subtract two scalar double-precision floats.',
  mulsd: 'Multiply two scalar double-precision floats.',
  divsd: 'Divide two scalar double-precision floats.',
  movsd: 'Move a scalar double-precision float.',
  cvtsi2sd: 'Convert a signed integer to a double-precision float.',
  cvttsd2si: 'Convert a double-precision float to a signed integer (truncating).',
  ucomisd: 'Compare scalar double-precision floats and set flags.',
};

/** Look up a plain-English description for a mnemonic (case-insensitive). */
export function describeOpcode(mnemonic: string | undefined | null): string | undefined {
  if (!mnemonic) return undefined;
  return OPCODE_DESCRIPTIONS[mnemonic.trim().toLowerCase()];
}
