use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{utils::store_data, LOAD_TX_HASH_SYSCALL_NUMBER, SUCCESS},
};
use ckb_types::{packed::Byte32, prelude::*};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct LoadTxHash {
    tx_hash: Byte32,
}

impl LoadTxHash {
    pub fn new(tx_hash: Byte32) -> LoadTxHash {
        LoadTxHash { tx_hash }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for LoadTxHash {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_TX_HASH_SYSCALL_NUMBER {
            return Ok(false);
        }

        let data = self.tx_hash.as_slice();
        let wrote_size = store_data(machine, data)?;

        machine.add_cycles(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
