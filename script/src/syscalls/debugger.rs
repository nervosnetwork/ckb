use crate::types::{
    DebugContext, DebugPrinter, {SgData, SgInfo},
};
use crate::{cost_model::transferred_byte_cycles, syscalls::DEBUG_PRINT_SYSCALL_NUMBER};
use ckb_vm::{
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
    registers::{A0, A7},
};
use std::sync::Arc;

pub struct Debugger {
    sg_info: Arc<SgInfo>,
    printer: DebugPrinter,
}

impl Debugger {
    pub fn new<DL>(sg_data: &SgData<DL>, debug_context: &DebugContext) -> Debugger {
        Debugger {
            sg_info: Arc::clone(&sg_data.sg_info),
            printer: Arc::clone(&debug_context.debug_printer),
        }
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
            .map_err(|e| VMError::External(format!("String from buffer {e:?}")))?;
        (self.printer)(&self.sg_info.script_hash, s.as_str());

        Ok(true)
    }
}
