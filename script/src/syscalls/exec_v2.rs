use crate::syscalls::{EXEC, INDEX_OUT_OF_BOUND};
use crate::types::{DataLocation, DataPieceId, ExecV2Args, Message, VmContext, VmData, VmId};
use ckb_traits::CellDataProvider;
use ckb_vm::{
    registers::{A0, A1, A2, A3, A4, A5, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

pub struct ExecV2 {
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl ExecV2 {
    pub fn new<DL: CellDataProvider>(
        vm_data: &Arc<VmData<DL>>,
        vm_context: &VmContext<DL>,
    ) -> ExecV2 {
        ExecV2 {
            id: vm_data.vm_id,
            message_box: Arc::clone(&vm_context.message_box),
        }
    }
}

impl<Mac> Syscalls<Mac> for ExecV2
where
    Mac: SupportMachine,
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != EXEC {
            return Ok(false);
        }
        let index = machine.registers()[A0].to_u64();
        let source = machine.registers()[A1].to_u64();
        let place = machine.registers()[A2].to_u64();
        let data_piece_id = match DataPieceId::try_from((source, index, place)) {
            Ok(id) => id,
            Err(_) => {
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(true);
            }
        };
        let bounds = machine.registers()[A3].to_u64();
        let offset = bounds >> 32;
        let length = bounds as u32 as u64;

        let argc = machine.registers()[A4].to_u64();
        let argv = machine.registers()[A5].to_u64();
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::ExecV2(
                self.id,
                ExecV2Args {
                    location: DataLocation {
                        data_piece_id,
                        offset,
                        length,
                    },
                    argc,
                    argv,
                },
            ));
        Err(VMError::Yield)
    }
}
