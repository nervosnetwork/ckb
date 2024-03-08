use crate::syscalls::SPAWN_EXTRA_CYCLES_BASE;
use crate::{
    v2_types::{DataPieceId, Message, PipeId, SpawnArgs, TxData, VmId},
    ScriptVersion,
};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    bytes::Bytes,
    machine::SupportMachine,
    memory::{Memory, FLAG_EXECUTABLE, FLAG_FREEZED},
    registers::{A0, A1, A2, A3, A4, A5, A7},
    snapshot2::{DataSource, Snapshot2Context},
    syscalls::Syscalls,
    Error, Register,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MachineContext<
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
> {
    pub(crate) id: VmId,
    pub(crate) base_cycles: Arc<Mutex<u64>>,
    pub(crate) message_box: Arc<Mutex<Vec<Message>>>,
    pub(crate) snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    pub(crate) script_version: ScriptVersion,
}

impl<DL> MachineContext<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub fn new(
        id: VmId,
        message_box: Arc<Mutex<Vec<Message>>>,
        tx_data: TxData<DL>,
        script_version: ScriptVersion,
    ) -> Self {
        Self {
            id,
            base_cycles: Arc::new(Mutex::new(0)),
            message_box,
            snapshot2_context: Arc::new(Mutex::new(Snapshot2Context::new(tx_data))),
            script_version,
        }
    }

    pub fn snapshot2_context(&self) -> &Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>> {
        &self.snapshot2_context
    }

    pub fn set_base_cycles(&mut self, base_cycles: u64) {
        *self.base_cycles.lock().expect("lock") = base_cycles;
    }

    // Reimplementation of load_cell_data but keep tracks of pages that are copied from
    // surrounding transaction data. Those pages do not need to be added to snapshots.
    fn load_cell_data<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let index = machine.registers()[A3].to_u64();
        let source = machine.registers()[A4].to_u64();

        let data_piece_id = match DataPieceId::try_from((source, index, 0)) {
            Ok(id) => id,
            Err(e) => {
                // Current implementation would throw an error immediately
                // for some source values, but return INDEX_OUT_OF_BOUND error
                // for other values. Here for simplicity, we would return
                // INDEX_OUT_OF_BOUND error in all cases. But the code might
                // differ to mimic current on-chain behavior
                println!("DataPieceId parsing error: {:?}", e);
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
        };

        let addr = machine.registers()[A0].to_u64();
        let size_addr = machine.registers()[A1].clone();
        let size = machine.memory_mut().load64(&size_addr)?.to_u64();
        let offset = machine.registers()[A2].to_u64();

        let mut sc = self.snapshot2_context().lock().expect("lock");
        let (wrote_size, full_size) =
            match sc.store_bytes(machine, addr, &data_piece_id, offset, size) {
                Ok(val) => val,
                Err(Error::External(m)) if m == "INDEX_OUT_OF_BOUND" => {
                    // This comes from TxData results in an out of bound error, to
                    // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                    machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                    return Ok(());
                }
                Err(e) => return Err(e),
            };

        machine
            .memory_mut()
            .store64(&size_addr, &Mac::REG::from_u64(full_size))?;
        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(())
    }

    // Reimplementation of load_cell_data_as_code but keep tracks of pages that are copied from
    // surrounding transaction data. Those pages do not need to be added to snapshots.
    //
    // Different from load_cell_data, this method showcases advanced usage of Snapshot2, where
    // one manually does the actual memory copying, then calls track_pages method to setup metadata
    // used by Snapshot2. It does not rely on higher level methods provided by Snapshot2.
    fn load_cell_data_as_code<Mac: SupportMachine>(
        &mut self,
        machine: &mut Mac,
    ) -> Result<(), Error> {
        let addr = machine.registers()[A0].to_u64();
        let memory_size = machine.registers()[A1].to_u64();
        let content_offset = machine.registers()[A2].to_u64();
        let content_size = machine.registers()[A3].to_u64();

        let index = machine.registers()[A4].to_u64();
        let source = machine.registers()[A5].to_u64();

        let data_piece_id = match DataPieceId::try_from((source, index, 0)) {
            Ok(id) => id,
            Err(e) => {
                // Current implementation would throw an error immediately
                // for some source values, but return INDEX_OUT_OF_BOUND error
                // for other values. Here for simplicity, we would return
                // INDEX_OUT_OF_BOUND error in all cases. But the code might
                // differ to mimic current on-chain behavior
                println!("DataPieceId parsing error: {:?}", e);
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
        };

        let mut sc = self.snapshot2_context().lock().expect("lock");
        // We are using 0..u64::max_value() to fetch full cell, there is
        // also no need to keep the full length value. Since cell's length
        // is already full length.
        let (cell, _) = match sc
            .data_source()
            .load_data(&data_piece_id, 0, u64::max_value())
        {
            Ok(val) => val,
            Err(Error::External(m)) if m == "INDEX_OUT_OF_BOUND" => {
                // This comes from TxData results in an out of bound error, to
                // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        let content_end = content_offset
            .checked_add(content_size)
            .ok_or(Error::MemOutOfBound)?;
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

    // Reimplementing debug syscall for printing debug messages
    fn debug<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
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
            .map_err(|e| Error::External(format!("String from buffer {e:?}")))?;
        println!("VM {}: {}", self.id, s);

        Ok(())
    }
}

impl<Mac: SupportMachine, DL> Syscalls<Mac> for MachineContext<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), Error> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, Error> {
        let code = machine.registers()[A7].to_u64();
        match code {
            2091 => self.load_cell_data_as_code(machine),
            2092 => self.load_cell_data(machine),
            2177 => self.debug(machine),
            _ => return Ok(false),
        }?;
        Ok(true)
    }
}

// Below are all simple utilities copied over from ckb-script package to
// ease the implementation.

/// How many bytes can transfer when VM costs one cycle.
// 0.25 cycles per byte
const BYTES_PER_CYCLE: u64 = 4;

/// Calculates how many cycles spent to load the specified number of bytes.
pub(crate) fn transferred_byte_cycles(bytes: u64) -> u64 {
    // Compiler will optimize the divisin here to shifts.
    (bytes + BYTES_PER_CYCLE - 1) / BYTES_PER_CYCLE
}

pub(crate) const SUCCESS: u8 = 0;
pub(crate) const INDEX_OUT_OF_BOUND: u8 = 1;
pub(crate) const SLICE_OUT_OF_BOUND: u8 = 3;
pub(crate) const WAIT_FAILURE: u8 = 5;
pub(crate) const INVALID_PIPE: u8 = 6;
pub(crate) const OTHER_END_CLOSED: u8 = 7;
