use crate::syscalls::DEBUG_PRINT_SYSCALL_NUMBER;
use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A7};
use log::debug;

pub struct Debugger<'a> {
    prefix: &'a str,
}

impl<'a> Debugger<'a> {
    pub fn new(prefix: &'a str) -> Debugger<'a> {
        Debugger { prefix }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for Debugger<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        let number = machine.registers()[A7].to_u64();
        if number != DEBUG_PRINT_SYSCALL_NUMBER {
            return Ok(false);
        }

        let mut addr = machine.registers()[A0].to_usize();
        let mut buffer = Vec::new();

        loop {
            let byte = machine.memory_mut().load8(addr)?;
            if byte == 0 {
                break;
            }
            buffer.push(byte);
            addr += 1;
        }

        let s = String::from_utf8(buffer).map_err(|_| VMError::ParseError)?;
        debug!(target: "script", "{} DEBUG OUTPUT: {}", self.prefix, s);
        Ok(true)
    }
}
