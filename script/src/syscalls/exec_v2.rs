use crate::syscalls::{
    Place, Source, EXEC, INDEX_OUT_OF_BOUND, SLICE_OUT_OF_BOUND, SOURCE_ENTRY_MASK,
    SOURCE_GROUP_FLAG,
};
use crate::types::{DataLocation, DataPieceId, ExecV2Args, Message, TxData, VmId};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    registers::{A0, A1, A2, A3, A4, A5, A7},
    snapshot2::Snapshot2Context,
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

pub struct ExecV2<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
    snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
}

impl<DL> ExecV2<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub fn new(
        id: VmId,
        message_box: Arc<Mutex<Vec<Message>>>,
        snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    ) -> ExecV2<DL> {
        ExecV2 {
            id,
            message_box,
            snapshot2_context,
        }
    }
}

impl<Mac, DL> Syscalls<Mac> for ExecV2<DL>
where
    Mac: SupportMachine,
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != EXEC {
            return Ok(false);
        }
        let index = machine.registers()[A0].to_u64();
        let mut source = machine.registers()[A1].to_u64();
        let place = machine.registers()[A2].to_u64();
        // To keep compatible with the old behavior. When Source is wrong, a
        // Vm internal error should be returned.
        if let Source::Group(_) = Source::parse_from_u64(source)? {
            source = source & SOURCE_ENTRY_MASK | SOURCE_GROUP_FLAG;
        } else {
            source &= SOURCE_ENTRY_MASK;
        }
        // To keep compatible with the old behavior.
        Place::parse_from_u64(place)?;

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

        // We are fetching the actual cell here for some in-place validation
        let mut sc = self
            .snapshot2_context
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?;
        let (_, full_length) = match sc.load_data(&data_piece_id, 0, 0) {
            Ok(val) => val,
            Err(VMError::SnapshotDataLoadError) => {
                // This comes from TxData results in an out of bound error, to
                // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(true);
            }
            Err(e) => return Err(e),
        };
        if offset >= full_length {
            machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
            return Ok(true);
        }
        if length > 0 {
            // Both offset and length are <= u32::MAX, so offset.checked_add(length) will be always a Some.
            let end = offset.checked_add(length).ok_or(VMError::MemOutOfBound)?;
            if end > full_length {
                machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
                return Ok(true);
            }
        }

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
