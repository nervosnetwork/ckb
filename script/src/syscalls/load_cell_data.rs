use crate::types::{DataPieceId, VmContext, VmData};
use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        Source, INDEX_OUT_OF_BOUND, LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER,
        LOAD_CELL_DATA_SYSCALL_NUMBER, SLICE_OUT_OF_BOUND, SOURCE_ENTRY_MASK, SOURCE_GROUP_FLAG,
        SUCCESS,
    },
};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    memory::{Memory, FLAG_EXECUTABLE, FLAG_FREEZED},
    registers::{A0, A1, A2, A3, A4, A5, A7},
    snapshot2::Snapshot2Context,
    Bytes, Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

pub struct LoadCellData<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, Arc<VmData<DL>>>>>,
}

impl<DL> LoadCellData<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub fn new(vm_context: &VmContext<DL>) -> LoadCellData<DL> {
        LoadCellData {
            snapshot2_context: Arc::clone(&vm_context.snapshot2_context),
        }
    }

    fn load_data<Mac: SupportMachine>(&self, machine: &mut Mac) -> Result<(), VMError> {
        let index = machine.registers()[A3].to_u64();
        let mut source = machine.registers()[A4].to_u64();
        // To keep compatible with the old behavior. When Source is wrong, a
        // Vm internal error should be returned.
        if let Source::Group(_) = Source::parse_from_u64(source)? {
            source = source & SOURCE_ENTRY_MASK | SOURCE_GROUP_FLAG;
        } else {
            source &= SOURCE_ENTRY_MASK;
        }
        let data_piece_id = match DataPieceId::try_from((source, index, 0)) {
            Ok(id) => id,
            Err(_) => {
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
        };
        let addr = machine.registers()[A0].to_u64();
        let size_addr = machine.registers()[A1].clone();
        let offset = machine.registers()[A2].to_u64();
        let mut sc = self
            .snapshot2_context
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?;

        match sc.load_data(&data_piece_id, offset, u64::MAX) {
            Ok((cell, _)) => {
                let size = machine.memory_mut().load64(&size_addr)?.to_u64();
                if size == 0 {
                    machine
                        .memory_mut()
                        .store64(&size_addr, &Mac::REG::from_u64(cell.len() as u64))?;
                    machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
                    return Ok(());
                }
                let (wrote_size, _) = match sc.store_bytes(
                    machine,
                    addr,
                    &data_piece_id,
                    offset,
                    size,
                    size_addr.to_u64(),
                ) {
                    Ok(val) => val,
                    Err(VMError::SnapshotDataLoadError) => {
                        // This comes from TxData results in an out of bound error, to
                        // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                        machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                };
                machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
                machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
                return Ok(());
            }
            Err(VMError::SnapshotDataLoadError) => {
                // This comes from TxData results in an out of bound error, to
                // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }

    fn load_data_as_code<Mac: SupportMachine>(&self, machine: &mut Mac) -> Result<(), VMError> {
        let addr = machine.registers()[A0].to_u64();
        let memory_size = machine.registers()[A1].to_u64();
        let content_offset = machine.registers()[A2].to_u64();
        let content_size = machine.registers()[A3].to_u64();
        let index = machine.registers()[A4].to_u64();
        let mut source = machine.registers()[A5].to_u64();
        // To keep compatible with the old behavior. When Source is wrong, a
        // Vm internal error should be returned.
        if let Source::Group(_) = Source::parse_from_u64(source)? {
            source = source & SOURCE_ENTRY_MASK | SOURCE_GROUP_FLAG;
        } else {
            source &= SOURCE_ENTRY_MASK;
        }
        let data_piece_id = match DataPieceId::try_from((source, index, 0)) {
            Ok(id) => id,
            Err(_) => {
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
        };
        let mut sc = self
            .snapshot2_context
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?;
        // We are using 0..u64::MAX to fetch full cell, there is
        // also no need to keep the full length value. Since cell's length
        // is already full length.
        let (cell, _) = match sc.load_data(&data_piece_id, 0, u64::MAX) {
            Ok(val) => {
                if content_size == 0 {
                    (Bytes::new(), val.1)
                } else {
                    val
                }
            }
            Err(VMError::SnapshotDataLoadError) => {
                // This comes from TxData results in an out of bound error, to
                // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let content_end = content_offset
            .checked_add(content_size)
            .ok_or(VMError::MemOutOfBound)?;
        if content_offset >= cell.len() as u64
            || content_end > cell.len() as u64
            || content_size > memory_size
        {
            machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
            return Ok(());
        }
        machine.memory_mut().init_pages(
            addr,
            memory_size,
            FLAG_EXECUTABLE | FLAG_FREEZED,
            Some(cell.slice((content_offset as usize)..(content_end as usize))),
            0,
        )?;
        sc.track_pages(machine, addr, memory_size, &data_piece_id, content_offset)?;
        machine.add_cycles_no_checking(transferred_byte_cycles(memory_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(())
    }
}

impl<Mac: SupportMachine, DL> Syscalls<Mac> for LoadCellData<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let code = machine.registers()[A7].to_u64();
        if code == LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER {
            self.load_data_as_code(machine)?;
            return Ok(true);
        } else if code == LOAD_CELL_DATA_SYSCALL_NUMBER {
            self.load_data(machine)?;
            return Ok(true);
        }
        Ok(false)
    }
}
