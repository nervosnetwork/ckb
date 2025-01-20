use crate::{syscalls::CURRENT_CYCLES, types::VmContext};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct CurrentCycles {
    base: Arc<Mutex<u64>>,
}

impl CurrentCycles {
    pub fn new<DL>(vm_context: &VmContext<DL>) -> Self
    where
        DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
    {
        Self {
            base: Arc::clone(&vm_context.base_cycles),
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for CurrentCycles {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != CURRENT_CYCLES {
            return Ok(false);
        }
        let cycles = self
            .base
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .checked_add(machine.cycles())
            .ok_or(VMError::CyclesOverflow)?;
        machine.set_register(A0, Mac::REG::from_u64(cycles));
        Ok(true)
    }
}
