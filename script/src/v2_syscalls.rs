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
pub(crate) const MAX_VMS_SPAWNED: u8 = 8;
