use crate::types::{DebugContext, DebugPrinter, SgData};
use crate::{cost_model::transferred_byte_cycles, syscalls::DEBUG_PRINT_SYSCALL_NUMBER};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
};
use std::sync::Arc;

pub struct Debugger<DL> {
    sg_data: Arc<SgData<DL>>,
    printer: DebugPrinter,
}

impl<DL> Debugger<DL> {
    pub fn new(sg_data: &Arc<SgData<DL>>, debug_context: &DebugContext) -> Debugger<DL> {
        Debugger {
            sg_data: Arc::clone(sg_data),
            printer: Arc::clone(&debug_context.debug_printer),
        }
    }
}

impl<Mac: SupportMachine, DL: Send + Sync> Syscalls<Mac> for Debugger<DL> {
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
        (self.printer)(self.sg_data.current_script_hash(), s.as_str());

        Ok(true)
    }
}
