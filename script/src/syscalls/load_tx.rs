use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        utils::store_data, LOAD_TRANSACTION_SYSCALL_NUMBER, LOAD_TX_HASH_SYSCALL_NUMBER, SUCCESS,
    },
    types::SgData,
};
use ckb_types::{core::cell::ResolvedTransaction, prelude::*};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct LoadTx {
    rtx: Arc<ResolvedTransaction>,
}

impl LoadTx {
    pub fn new<DL>(sg_data: &SgData<DL>) -> LoadTx {
        LoadTx {
            rtx: Arc::clone(&sg_data.rtx),
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for LoadTx {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let wrote_size = match machine.registers()[A7].to_u64() {
            LOAD_TX_HASH_SYSCALL_NUMBER => {
                store_data(machine, self.rtx.transaction.hash().as_slice())?
            }
            LOAD_TRANSACTION_SYSCALL_NUMBER => {
                store_data(machine, self.rtx.transaction.data().as_slice())?
            }
            _ => return Ok(false),
        };

        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
