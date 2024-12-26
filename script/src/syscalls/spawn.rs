use crate::syscalls::utils::load_c_string;
use crate::syscalls::{
    Source, INDEX_OUT_OF_BOUND, MAX_ARGV_LENGTH, SLICE_OUT_OF_BOUND, SOURCE_ENTRY_MASK,
    SOURCE_GROUP_FLAG, SPAWN, SPAWN_EXTRA_CYCLES_BASE, SPAWN_YIELD_CYCLES_BASE,
};
use crate::types::{DataPieceId, Fd, Message, SpawnArgs, TxData, VmId};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::core::error::ARGV_TOO_LONG_TEXT;
use ckb_vm::{
    machine::SupportMachine,
    memory::Memory,
    registers::{A0, A1, A2, A3, A4, A7},
    snapshot2::Snapshot2Context,
    syscalls::Syscalls,
    Error as VMError, Register,
};
use std::sync::{Arc, Mutex};

pub struct Spawn<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
    snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
}

impl<DL> Spawn<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub fn new(
        id: VmId,
        message_box: Arc<Mutex<Vec<Message>>>,
        snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    ) -> Self {
        Self {
            id,
            message_box,
            snapshot2_context,
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
            .load64(&Mac::REG::from_u64(argc_addr))?
            .to_u64();
        let argv_addr_addr = spgs_addr.wrapping_add(8);
        let argv_addr = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(argv_addr_addr))?
            .to_u64();
        let mut addr = argv_addr;
        let mut argv = Vec::new();
        let mut argv_length: u64 = 0;
        for _ in 0..argc {
            let target_addr = machine
                .memory_mut()
                .load64(&Mac::REG::from_u64(addr))?
                .to_u64();
            let cstr = load_c_string(machine, target_addr)?;
            let cstr_len = cstr.len();
            argv.push(cstr);

            // Number of argv entries should also be considered
            argv_length = argv_length
                .saturating_add(8)
                .saturating_add(cstr_len as u64);
            if argv_length > MAX_ARGV_LENGTH {
                return Err(VMError::Unexpected(ARGV_TOO_LONG_TEXT.to_string()));
            }

            addr = addr.wrapping_add(8);
        }

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
                    data_piece_id,
                    offset,
                    length,
                    argv,
                    fds,
                    process_id_addr,
                },
            ));
        Err(VMError::Yield)
    }
}
