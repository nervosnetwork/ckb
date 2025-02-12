use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{utils::store_data, LOAD_SCRIPT_SYSCALL_NUMBER, SUCCESS},
    types::SgData,
};
use ckb_types::prelude::*;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct LoadScript<DL> {
    sg_data: Arc<SgData<DL>>,
}

impl<DL> LoadScript<DL> {
    pub fn new(sg_data: &Arc<SgData<DL>>) -> Self {
        Self {
            sg_data: Arc::clone(sg_data),
        }
    }
}

impl<Mac: SupportMachine, DL: Send + Sync> Syscalls<Mac> for LoadScript<DL> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_SCRIPT_SYSCALL_NUMBER {
            return Ok(false);
        }

        let data = self.sg_data.script_group.script.as_slice();
        let wrote_size = store_data(machine, data)?;

        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
