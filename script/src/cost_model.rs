//! CKB VM cost model.
//!
//! The cost model assign cycles to instructions.
use ckb_vm::{
    instructions::{extract_opcode, insts},
    Instruction,
};

/// How many bytes can transfer when VM costs one cycle.
// 0.25 cycles per byte
pub const BYTES_PER_CYCLE: u64 = 4;

/// The cost of switching between assembly and rust.
pub const CONTEXT_SWITCH_CYCLE: u64 = 500;

/// Calculates how many cycles spent to load the specified number of bytes.
pub fn transferred_byte_cycles(bytes: u64) -> u64 {
    // Compiler will optimize the divisin here to shifts.
    (bytes + BYTES_PER_CYCLE - 1) / BYTES_PER_CYCLE
}

/// Returns the spent cycles to execute the secific instruction.
pub fn instruction_cycles(i: Instruction) -> u64 {
    match extract_opcode(i) {
        // IMC
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
        insts::OP_EBREAK => CONTEXT_SWITCH_CYCLE,
        insts::OP_ECALL => CONTEXT_SWITCH_CYCLE,
        insts::OP_JAL => 3,
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
        // B
        insts::OP_GREV => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_GREVI => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_GREVW => CONTEXT_SWITCH_CYCLE + 18,
        insts::OP_GREVIW => CONTEXT_SWITCH_CYCLE + 18,
        insts::OP_SHFL => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_UNSHFL => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_SHFLI => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_UNSHFLI => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_SHFLW => CONTEXT_SWITCH_CYCLE + 18,
        insts::OP_UNSHFLW => CONTEXT_SWITCH_CYCLE + 18,
        insts::OP_GORC => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_GORCI => CONTEXT_SWITCH_CYCLE + 20,
        insts::OP_GORCW => CONTEXT_SWITCH_CYCLE + 18,
        insts::OP_GORCIW => CONTEXT_SWITCH_CYCLE + 18,
        insts::OP_BFP => CONTEXT_SWITCH_CYCLE + 15,
        insts::OP_BFPW => CONTEXT_SWITCH_CYCLE + 15,
        insts::OP_BDEP => CONTEXT_SWITCH_CYCLE + 350,
        insts::OP_BEXT => CONTEXT_SWITCH_CYCLE + 270,
        insts::OP_BDEPW => CONTEXT_SWITCH_CYCLE + 180,
        insts::OP_BEXTW => CONTEXT_SWITCH_CYCLE + 140,
        insts::OP_CLMUL => CONTEXT_SWITCH_CYCLE + 320,
        insts::OP_CLMULR => CONTEXT_SWITCH_CYCLE + 380,
        insts::OP_CLMULH => CONTEXT_SWITCH_CYCLE + 400,
        insts::OP_CLMULW => CONTEXT_SWITCH_CYCLE + 60,
        insts::OP_CLMULRW => CONTEXT_SWITCH_CYCLE + 60,
        insts::OP_CLMULHW => CONTEXT_SWITCH_CYCLE + 60,
        insts::OP_CRC32B => CONTEXT_SWITCH_CYCLE + 15,
        insts::OP_CRC32H => CONTEXT_SWITCH_CYCLE + 30,
        insts::OP_CRC32W => CONTEXT_SWITCH_CYCLE + 45,
        insts::OP_CRC32D => CONTEXT_SWITCH_CYCLE + 60,
        insts::OP_CRC32CB => CONTEXT_SWITCH_CYCLE + 15,
        insts::OP_CRC32CH => CONTEXT_SWITCH_CYCLE + 30,
        insts::OP_CRC32CW => CONTEXT_SWITCH_CYCLE + 45,
        insts::OP_CRC32CD => CONTEXT_SWITCH_CYCLE + 60,
        insts::OP_BMATFLIP => CONTEXT_SWITCH_CYCLE + 40,
        insts::OP_BMATOR => CONTEXT_SWITCH_CYCLE + 500,
        insts::OP_BMATXOR => CONTEXT_SWITCH_CYCLE + 800,
        // MOP
        insts::OP_WIDE_MUL => 5,
        insts::OP_WIDE_MULU => 5,
        insts::OP_WIDE_DIV => 32,
        insts::OP_WIDE_DIVU => 32,
        insts::OP_FAR_JUMP_REL => 3,
        insts::OP_FAR_JUMP_ABS => 3,
        _ => 1,
    }
}
