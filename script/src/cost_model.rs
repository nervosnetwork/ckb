//! CKB VM cost model.
//!
//! The cost model assign cycles to instructions.

/// How many bytes can transfer when VM costs one cycle.
// 0.25 cycles per byte
pub const BYTES_PER_CYCLE: u64 = 4;

/// Calculates how many cycles spent to load the specified number of bytes.
pub fn transferred_byte_cycles(bytes: u64) -> u64 {
    // Compiler will optimize the divisin here to shifts.
    (bytes + BYTES_PER_CYCLE - 1) / BYTES_PER_CYCLE
}
