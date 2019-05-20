use crate::syscalls::{utils::store_data, LOAD_SCRIPT_HASH_SYSCALL_NUMBER, SUCCESS};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct LoadScriptHash<'a> {
    hash: &'a [u8],
}

impl<'a> LoadScriptHash<'a> {
    pub fn new(hash: &'a [u8]) -> LoadScriptHash<'a> {
        LoadScriptHash { hash }
    }
}

impl<'a, Mac: SupportMachine> Syscalls<Mac> for LoadScriptHash<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_SCRIPT_HASH_SYSCALL_NUMBER {
            return Ok(false);
        }

        store_data(machine, &self.hash)?;

        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(self.hash.len() as u64 * 10)?;
        Ok(true)
    }
}
