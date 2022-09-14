use crate::{cost_model::transferred_byte_cycles, syscalls::DEBUG_PRINT_SYSCALL_NUMBER};
use ckb_types::packed::Byte32;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
};
use std::rc::Rc;

pub struct Debugger {
    hash: Byte32,
    printer: Rc<dyn Fn(&Byte32, &str)>,
}

impl Debugger {
    pub fn new(hash: Byte32, printer: Rc<dyn Fn(&Byte32, &str)>) -> Debugger {
        Debugger { hash, printer }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Debugger {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let number = machine.registers()[A7].to_u64();
        if number != DEBUG_PRINT_SYSCALL_NUMBER {
            return Ok(false);
        }

        let mut addr = machine.registers()[A0].to_u64();
        let mut buffer = Vec::new();

        loop {
            let byte = machine
                .memory_mut()
                .load8(&Mac::REG::from_u64(addr))?
                .to_u8();
            if byte == 0 {
                break;
            }
            buffer.push(byte);
            addr += 1;
        }

        machine.add_cycles_no_checking(transferred_byte_cycles(buffer.len() as u64))?;
        let s = String::from_utf8(buffer)
            .map_err(|e| VMError::External(format!("String from buffer {:?}", e)))?;
        (self.printer)(&self.hash, s.as_str());

        Ok(true)
    }
}
