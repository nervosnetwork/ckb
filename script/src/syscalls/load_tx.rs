use crate::syscalls::{LOAD_TX_SYSCALL_NUMBER, SUCCESS};
use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A1, A2, A7};
use std::cmp;

pub struct LoadTx<'a> {
    tx: &'a [u8],
}

impl<'a> LoadTx<'a> {
    pub fn new(tx: &'a [u8]) -> LoadTx<'a> {
        LoadTx { tx }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for LoadTx<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_TX_SYSCALL_NUMBER {
            return Ok(false);
        }

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let size = machine.memory_mut().load64(size_addr)? as usize;

        let data = self.tx;

        let offset = machine.registers()[A2].to_usize();
        let full_size = data.len() - offset;
        let real_size = cmp::min(size, full_size);
        machine.memory_mut().store64(size_addr, full_size as u64)?;
        machine
            .memory_mut()
            .store_bytes(addr, &data[offset..offset + real_size])?;
        machine.registers_mut()[A0] = R::from_u8(SUCCESS);
        machine.add_cycles((data.len() as u64 + 1) * 10);
        Ok(true)
    }
}
