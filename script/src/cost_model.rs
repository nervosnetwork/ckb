//! TODO(doc): @doitian
use ckb_vm::{
    instructions::{extract_opcode, insts},
    Instruction,
};

/// TODO(doc): @doitian
// 0.25 cycles per byte
pub const BYTES_PER_CYCLE: u64 = 4;

/// TODO(doc): @doitian
pub fn transferred_byte_cycles(bytes: u64) -> u64 {
    // Compiler will optimize the divisin here to shifts.
    (bytes + BYTES_PER_CYCLE - 1) / BYTES_PER_CYCLE
}

/// TODO(doc): @doitian
pub fn instruction_cycles(i: Instruction) -> u64 {
    match extract_opcode(i) {
        insts::OP_JALR => 3,
        insts::OP_LD => 2,
        insts::OP_LW => 3,
        insts::OP_LH => 3,
        insts::OP_LB => 3,
        insts::OP_LWU => 3,
        insts::OP_LHU => 3,
        insts::OP_LBU => 3,
        insts::OP_SB => 3,
        insts::OP_SH => 3,
        insts::OP_SW => 3,
        insts::OP_SD => 2,
        insts::OP_BEQ => 3,
        insts::OP_BGE => 3,
        insts::OP_BGEU => 3,
        insts::OP_BLT => 3,
        insts::OP_BLTU => 3,
        insts::OP_BNE => 3,
        insts::OP_EBREAK => 500,
        insts::OP_ECALL => 500,
        insts::OP_JAL => 3,
        insts::OP_RVC_LW => 3,
        insts::OP_RVC_LD => 2,
        insts::OP_RVC_SW => 3,
        insts::OP_RVC_SD => 2,
        insts::OP_RVC_LWSP => 3,
        insts::OP_RVC_LDSP => 2,
        insts::OP_RVC_SWSP => 3,
        insts::OP_RVC_SDSP => 2,
        insts::OP_RVC_BEQZ => 3,
        insts::OP_RVC_BNEZ => 3,
        insts::OP_RVC_JAL => 3,
        insts::OP_RVC_J => 3,
        insts::OP_RVC_JR => 3,
        insts::OP_RVC_JALR => 3,
        insts::OP_RVC_EBREAK => 500,
        insts::OP_MUL => 5,
        insts::OP_MULW => 5,
        insts::OP_MULH => 5,
        insts::OP_MULHU => 5,
        insts::OP_MULHSU => 5,
        insts::OP_DIV => 32,
        insts::OP_DIVW => 32,
        insts::OP_DIVU => 32,
        insts::OP_DIVUW => 32,
        insts::OP_REM => 32,
        insts::OP_REMW => 32,
        insts::OP_REMU => 32,
        insts::OP_REMUW => 32,
        _ => 1,
    }
}
