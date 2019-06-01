use crate::syscalls::{utils::store_data, LOAD_TX_HASH_SYSCALL_NUMBER, SUCCESS};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct LoadTxHash<'a> {
    tx_hash: &'a [u8],
}

impl<'a> LoadTxHash<'a> {
    pub fn new(tx_hash: &'a [u8]) -> LoadTxHash<'a> {
        LoadTxHash { tx_hash }
    }
}

impl<'a, Mac: SupportMachine> Syscalls<Mac> for LoadTxHash<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_TX_HASH_SYSCALL_NUMBER {
            return Ok(false);
        }

        store_data(machine, &self.tx_hash)?;

        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(self.tx_hash.len() as u64 * 10)?;
        Ok(true)
    }
}
