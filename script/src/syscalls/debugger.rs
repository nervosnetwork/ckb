use crate::syscalls::DEBUG_PRINT_SYSCALL_NUMBER;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
};

pub struct Debugger<'a> {
    printer: &'a Fn(&str),
}

impl<'a> Debugger<'a> {
    pub fn new(printer: &'a Fn(&str)) -> Debugger<'a> {
        Debugger { printer }
    }
}

impl<'a, Mac: SupportMachine> Syscalls<Mac> for Debugger<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let number = machine.registers()[A7].to_u64();
        if number != DEBUG_PRINT_SYSCALL_NUMBER {
            return Ok(false);
        }

        let mut addr = machine.registers()[A0].to_usize();
        let mut buffer = Vec::new();

        loop {
            let byte = machine
                .memory_mut()
                .load8(&Mac::REG::from_usize(addr))?
                .to_u8();
            if byte == 0 {
                break;
            }
            buffer.push(byte);
            addr += 1;
        }

        machine.add_cycles(buffer.len() as u64 * 10)?;
        let s = String::from_utf8(buffer).map_err(|_| VMError::ParseError)?;
        (self.printer)(s.as_str());

        Ok(true)
    }
}
