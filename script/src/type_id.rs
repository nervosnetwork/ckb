use crate::{ScriptError, ScriptGroup};
use ckb_hash::new_blake2b;
use ckb_types::{
    core::{cell::ResolvedTransaction, Cycle},
    prelude::*,
};

// NOTE: we give this special TYPE_ID script a large cycle on purpose. This way
// we can ensure that the special built-in TYPE_ID script here only exists for
// safety, not for saving cycles. In fact if you want to optimize for the cycle
// consumptions, you should implement the TYPE_ID script as a real script, which
// will use far less cycles. This way we can ensure that we won't run into
// situations in similar chains that developers yearn for builtin contracts
// which can have far less gas/cycle consumptions than one implemented in native
// bytecode support by that chain.
pub const TYPE_ID_CYCLES: Cycle = 1_000_000;

pub const ERROR_ARGS: i8 = -1;
pub const ERROR_TOO_MANY_CELLS: i8 = -2;
pub const ERROR_INVALID_INPUT_HASH: i8 = -3;

pub struct TypeIdSystemScript<'a> {
    pub rtx: &'a ResolvedTransaction,
    pub script_group: &'a ScriptGroup,
    pub max_cycles: Cycle,
}

impl<'a> TypeIdSystemScript<'a> {
    pub fn verify(&self) -> Result<Cycle, ScriptError> {
        if self.max_cycles < TYPE_ID_CYCLES {
            return Err(ScriptError::ExceededMaximumCycles(self.max_cycles));
        }
        // TYPE_ID script should only accept one argument,
        // which is the hash of all inputs when creating
        // the cell.
        if self.script_group.script.args().len() != 32 {
            return Err(ScriptError::ValidationFailure(ERROR_ARGS));
        }

        // There could be at most one input cell and one
        // output cell with current TYPE_ID script.
        if self.script_group.input_indices.len() > 1 || self.script_group.output_indices.len() > 1 {
            return Err(ScriptError::ValidationFailure(ERROR_TOO_MANY_CELLS));
        }

        // If there's only one output cell with current
        // TYPE_ID script, we are creating such a cell,
        // we also need to validate that the first argument matches
        // the hash of following items concatenated:
        // 1. First CellInput of the transaction.
        // 2. Index of the first output cell in current script group.
        if self.script_group.input_indices.is_empty() {
            let first_cell_input = self
                .rtx
                .transaction
                .inputs()
                .get(0)
                .ok_or(ScriptError::ValidationFailure(ERROR_ARGS))?;
            let first_output_index: u64 = self
                .script_group
                .output_indices
                .get(0)
                .map(|output_index| *output_index as u64)
                .ok_or(ScriptError::ValidationFailure(ERROR_ARGS))?;

            let mut blake2b = new_blake2b();
            blake2b.update(first_cell_input.as_slice());
            blake2b.update(&first_output_index.to_le_bytes());
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);

            if ret[..] != self.script_group.script.args().raw_data()[..] {
                return Err(ScriptError::ValidationFailure(ERROR_INVALID_INPUT_HASH));
            }
        }
        Ok(TYPE_ID_CYCLES)
    }
}
