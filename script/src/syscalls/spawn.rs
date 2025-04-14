use crate::syscalls::{
    INDEX_OUT_OF_BOUND, SLICE_OUT_OF_BOUND, SOURCE_ENTRY_MASK, SOURCE_GROUP_FLAG, SPAWN,
    SPAWN_EXTRA_CYCLES_BASE, SPAWN_YIELD_CYCLES_BASE, Source,
};
use crate::types::{DataLocation, DataPieceId, Fd, Message, SgData, SpawnArgs, VmContext, VmId};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    Error as VMError, Register,
    machine::SupportMachine,
    memory::Memory,
    registers::{A0, A1, A2, A3, A4, A7},
    snapshot2::Snapshot2Context,
    syscalls::Syscalls,
};
use std::sync::{Arc, Mutex};

pub struct Spawn<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
    snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, SgData<DL>>>>,
}

impl<DL> Spawn<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub fn new(vm_id: &VmId, vm_context: &VmContext<DL>) -> Self {
        Self {
            id: *vm_id,
            message_box: Arc::clone(&vm_context.message_box),
            snapshot2_context: Arc::clone(&vm_context.snapshot2_context),
        }
    }
}

impl<Mac, DL> Syscalls<Mac> for Spawn<DL>
where
    Mac: SupportMachine,
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != SPAWN {
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
        let spgs_addr = machine.registers()[A4].to_u64();
        let argc_addr = spgs_addr;
        let argc = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(argc_addr))?;
        let argv_addr = spgs_addr.wrapping_add(8);
        let argv = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(argv_addr))?;
        let process_id_addr_addr = spgs_addr.wrapping_add(16);
        let process_id_addr = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(process_id_addr_addr))?
            .to_u64();
        let fds_addr_addr = spgs_addr.wrapping_add(24);
        let mut fds_addr = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(fds_addr_addr))?
            .to_u64();

        let mut fds = vec![];
        if fds_addr != 0 {
            loop {
                let fd = machine
                    .memory_mut()
                    .load64(&Mac::REG::from_u64(fds_addr))?
                    .to_u64();
                if fd == 0 {
                    break;
                }
                fds.push(Fd(fd));
                fds_addr += 8;
            }
        }

        // We are fetching the actual cell here for some in-place validation
        let sc = self
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
            let end = offset.checked_add(length).ok_or(VMError::MemOutOfBound)?;
            if end > full_length {
                machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
                return Ok(true);
            }
        }
        machine.add_cycles_no_checking(SPAWN_EXTRA_CYCLES_BASE)?;
        machine.add_cycles_no_checking(SPAWN_YIELD_CYCLES_BASE)?;
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::Spawn(
                self.id,
                SpawnArgs {
                    location: DataLocation {
                        data_piece_id,
                        offset,
                        length,
                    },
                    argc: argc.to_u64(),
                    argv: argv.to_u64(),
                    fds,
                    process_id_addr,
                },
            ));
        Err(VMError::Yield)
    }
}
