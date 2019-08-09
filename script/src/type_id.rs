use crate::ScriptGroup;
use byteorder::{ByteOrder, LittleEndian};
use ckb_core::cell::ResolvedTransaction;
use ckb_core::Cycle;
use ckb_error::{Error, ScriptError};
use ckb_hash::new_blake2b;
use numext_fixed_hash::{h256, H256};

// "TYPE_ID" in hex
pub const TYPE_ID_CODE_HASH: H256 = h256!("0x545950455f4944");
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
    pub rtx: &'a ResolvedTransaction<'a>,
    pub script_group: &'a ScriptGroup,
    pub max_cycles: Cycle,
}

impl<'a> TypeIdSystemScript<'a> {
    pub fn verify(&self) -> Result<Cycle, Error> {
        if self.max_cycles < TYPE_ID_CYCLES {
            Err(ScriptError::TooMuchCycles)?;
        }
        // TYPE_ID script should only accept one argument,
        // which is the hash of all inputs when creating
        // the cell.
        if self.script_group.script.args.len() != 1 || self.script_group.script.args[0].len() != 32
        {
            Err(ScriptError::ValidationFailure(ERROR_ARGS))?;
        }

        // There could be at most one input cell and one
        // output cell with current TYPE_ID script.
        if self.script_group.input_indices.len() > 1 || self.script_group.output_indices.len() > 1 {
            Err(ScriptError::ValidationFailure(ERROR_TOO_MANY_CELLS))?;
        }

        // If there's only one output cell with current
        // TYPE_ID script, we are creating such a cell,
        // we also need to validate that the hash of all
        // inputs match the first argument of the script.
        if self.script_group.input_indices.is_empty() {
            let mut blake2b = new_blake2b();
            for input in self.rtx.transaction.inputs() {
                // TODO: we use this weird way of hashing data to avoid
                // dependency on flatbuffers for now. We should change
                // this when we have a better serialization solution.
                if let Some(cell) = &input.previous_output.cell {
                    blake2b.update(b"cell");
                    blake2b.update(cell.tx_hash.as_bytes());
                    let mut buf = [0; 4];
                    LittleEndian::write_u32(&mut buf, cell.index);
                    blake2b.update(&buf[..]);
                }
                if let Some(block_hash) = &input.previous_output.block_hash {
                    blake2b.update(b"block_hash");
                    blake2b.update(block_hash.as_bytes());
                }
                blake2b.update(b"since");
                let mut buf = [0; 8];
                LittleEndian::write_u64(&mut buf, input.since);
                blake2b.update(&buf[..]);
            }
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            if ret[..] != self.script_group.script.args[0] {
                Err(ScriptError::ValidationFailure(ERROR_INVALID_INPUT_HASH))?;
            }
        }
        Ok(TYPE_ID_CYCLES)
    }
}
