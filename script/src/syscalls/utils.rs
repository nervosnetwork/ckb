use byteorder::{ByteOrder, LittleEndian};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    registers::{A0, A1, A2},
    snapshot2::Snapshot2Context,
    Error as VMError, Memory, Register, SupportMachine,
};
use std::cmp;
use std::sync::{Arc, Mutex};

use crate::syscalls::{INDEX_OUT_OF_BOUND, SLICE_OUT_OF_BOUND};
use crate::types::TxData;
use crate::DataPieceId;

pub fn store_data<Mac: SupportMachine>(machine: &mut Mac, data: &[u8]) -> Result<u64, VMError> {
    let addr = machine.registers()[A0].to_u64();
    let size_addr = machine.registers()[A1].clone();
    let data_len = data.len() as u64;
    let offset = cmp::min(data_len, machine.registers()[A2].to_u64());

    let size = machine.memory_mut().load64(&size_addr)?.to_u64();
    let full_size = data_len - offset;
    let real_size = cmp::min(size, full_size);
    machine
        .memory_mut()
        .store64(&size_addr, &Mac::REG::from_u64(full_size))?;
    machine
        .memory_mut()
        .store_bytes(addr, &data[offset as usize..(offset + real_size) as usize])?;
    Ok(real_size)
}

pub fn store_u64<Mac: SupportMachine>(machine: &mut Mac, v: u64) -> Result<u64, VMError> {
    let mut buffer = [0u8; std::mem::size_of::<u64>()];
    LittleEndian::write_u64(&mut buffer, v);
    store_data(machine, &buffer)
}

pub fn validate_offset_length<Mac, DL>(
    machine: &mut Mac,
    snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    data_piece_id: &DataPieceId,
    offset: u64,
    length: u64,
) -> Result<bool, VMError>
where
    Mac: SupportMachine,
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    let mut sc = snapshot2_context
        .lock()
        .map_err(|e| VMError::Unexpected(e.to_string()))?;
    let (_, full_length) = match sc.load_data(&data_piece_id, 0, 0) {
        Ok(val) => val,
        Err(VMError::SnapshotDataLoadError) => {
            // This comes from TxData results in an out of bound error, to
            // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
            machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    if offset >= full_length {
        machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
        return Ok(false);
    }
    if length > 0 {
        let end = offset.checked_add(length).ok_or(VMError::MemOutOfBound)?;
        if end > full_length {
            machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
            return Ok(false);
        }
    }
    Ok(true)
}
