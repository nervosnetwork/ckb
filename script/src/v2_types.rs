use crate::ScriptGroup;
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::core::{cell::ResolvedTransaction, Cycle};
use ckb_vm::{
    bytes::Bytes,
    machine::Pause,
    snapshot2::{DataSource, Snapshot2},
    Error, RISCV_GENERAL_REGISTER_NUMBER,
};
use std::mem::size_of;
use std::sync::Arc;

pub type VmId = u64;

pub const FIRST_VM_ID: VmId = 0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipeId(pub(crate) u64);

pub const FIRST_PIPE_SLOT: u64 = 2;

impl PipeId {
    pub fn create(slot: u64) -> (PipeId, PipeId, u64) {
        (PipeId(slot), PipeId(slot + 1), slot + 2)
    }

    pub fn other_pipe(&self) -> PipeId {
        PipeId(self.0 ^ 0x1)
    }

    pub fn is_read(&self) -> bool {
        self.0 % 2 == 0
    }

    pub fn is_write(&self) -> bool {
        self.0 % 2 == 1
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VmState {
    Runnable,
    Terminated,
    Wait {
        target_vm_id: VmId,
        exit_code_addr: u64,
    },
    WaitForWrite {
        pipe: PipeId,
        consumed: u64,
        length: u64,
        buffer_addr: u64,
        length_addr: u64,
    },
    WaitForRead {
        pipe: PipeId,
        length: u64,
        buffer_addr: u64,
        length_addr: u64,
    },
}

#[derive(Clone, Debug)]
pub struct SpawnArgs {
    pub data_piece_id: DataPieceId,
    pub offset: u64,
    pub length: u64,
    pub argv: Vec<Bytes>,
    pub pipes: Vec<PipeId>,
    pub process_id_addr: u64,
}

#[derive(Clone, Debug)]
pub struct WaitArgs {
    pub target_id: VmId,
    pub exit_code_addr: u64,
}

#[derive(Clone, Debug)]
pub struct PipeArgs {
    pub pipe1_addr: u64,
    pub pipe2_addr: u64,
}

#[derive(Clone, Debug)]
pub struct PipeIoArgs {
    pub pipe: PipeId,
    pub length: u64,
    pub buffer_addr: u64,
    pub length_addr: u64,
}

#[derive(Clone, Debug)]
pub enum Message {
    Spawn(VmId, SpawnArgs),
    Wait(VmId, WaitArgs),
    Pipe(VmId, PipeArgs),
    PipeRead(VmId, PipeIoArgs),
    PipeWrite(VmId, PipeIoArgs),
    InheritedFileDescriptor(VmId, PipeIoArgs),
    Close(VmId, PipeId),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DataPieceId {
    Program,
    Input(u32),
    Output(u32),
    CellDep(u32),
    GroupInput(u32),
    GroupOutput(u32),
    Witness(u32),
    WitnessGroupInput(u32),
    WitnessGroupOutput(u32),
}

impl TryFrom<(u64, u64, u64)> for DataPieceId {
    type Error = String;

    fn try_from(value: (u64, u64, u64)) -> Result<Self, Self::Error> {
        let (source, index, place) = value;
        let index: u32 =
            u32::try_from(index).map_err(|e| format!("Error casting index to u32: {}", e))?;
        match (source, place) {
            (1, 0) => Ok(DataPieceId::Input(index)),
            (2, 0) => Ok(DataPieceId::Output(index)),
            (3, 0) => Ok(DataPieceId::CellDep(index)),
            (0x0100000000000001, 0) => Ok(DataPieceId::GroupInput(index)),
            (0x0100000000000002, 0) => Ok(DataPieceId::GroupOutput(index)),
            (1, 1) => Ok(DataPieceId::Witness(index)),
            (2, 1) => Ok(DataPieceId::Witness(index)),
            (0x0100000000000001, 1) => Ok(DataPieceId::WitnessGroupInput(index)),
            (0x0100000000000002, 1) => Ok(DataPieceId::WitnessGroupOutput(index)),
            _ => Err(format!("Invalid source value: {:#x}", source)),
        }
    }
}

/// Full state representing all VM instances from verifying a CKB script.
/// It should be serializable to binary formats, while also be able to
/// fully recover the running environment with the full transaction environment.
#[derive(Clone, Debug)]
pub struct FullSuspendedState {
    pub max_vms_count: u64,
    pub total_cycles: Cycle,
    pub next_vm_id: VmId,
    pub next_pipe_slot: u64,
    pub vms: Vec<(VmId, VmState, Snapshot2<DataPieceId>)>,
    pub pipes: Vec<(PipeId, VmId)>,
    pub inherited_fd: Vec<(VmId, Vec<PipeId>)>,
    pub terminated_vms: Vec<(VmId, i8)>,
}

impl FullSuspendedState {
    pub fn size(&self) -> u64 {
        (size_of::<Cycle>()
            + size_of::<VmId>()
            + size_of::<u64>()
            + self.vms.iter().fold(0, |mut acc, (_, _, snapshot)| {
                acc += size_of::<VmId>() + size_of::<VmState>();
                acc += snapshot.pages_from_source.len()
                    * (size_of::<u64>()
                        + size_of::<u8>()
                        + size_of::<DataPieceId>()
                        + size_of::<u64>()
                        + size_of::<u64>());
                for dirty_page in &snapshot.dirty_pages {
                    acc += size_of::<u64>() + size_of::<u8>() + dirty_page.2.len();
                }
                acc += size_of::<u32>()
                    + RISCV_GENERAL_REGISTER_NUMBER * size_of::<u64>()
                    + size_of::<u64>()
                    + size_of::<u64>()
                    + size_of::<u64>();
                acc
            })
            + (self.pipes.len() * (size_of::<PipeId>() + size_of::<VmId>()))) as u64
            + (self.inherited_fd.len() * (size_of::<PipeId>())) as u64
            + (self.terminated_vms.len() * (size_of::<VmId>() + size_of::<i8>())) as u64
    }
}

/// Context data for current running transaction & script
#[derive(Clone)]
pub struct TxData<DL> {
    pub rtx: Arc<ResolvedTransaction>,
    pub data_loader: DL,
    // Ideally one might not want to keep program here, since program is totally
    // deducible from rtx + data_loader, however, for a demo here, program
    // does help us save some extra coding.
    pub program: Bytes,
    pub script_group: Arc<ScriptGroup>,
}

impl<DL> DataSource<DataPieceId> for TxData<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    fn load_data(&self, id: &DataPieceId, offset: u64, length: u64) -> Result<(Bytes, u64), Error> {
        match id {
            DataPieceId::Program => {
                // This is just a shortcut so we don't have to copy over the logic in extract_script,
                // ideally you can also only define the rest 5, then figure out a way to convert
                // script group to the actual cell dep index.
                Ok(self.program.clone())
            }
            DataPieceId::Input(i) => {
                let cell = self
                    .rtx
                    .resolved_inputs
                    .get(*i as usize)
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))?;
                self.data_loader.load_cell_data(cell).ok_or_else(|| {
                    Error::Unexpected(format!("Loading input cell #{}'s data failed!", i))
                })
            }
            DataPieceId::Output(i) => self
                .rtx
                .transaction
                .outputs_data()
                .get(*i as usize)
                .map(|data| data.raw_data())
                .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string())),
            DataPieceId::CellDep(i) => {
                let cell = self
                    .rtx
                    .resolved_cell_deps
                    .get(*i as usize)
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))?;
                self.data_loader.load_cell_data(cell).ok_or_else(|| {
                    Error::Unexpected(format!("Loading dep cell #{}'s data failed!", i))
                })
            }
            DataPieceId::GroupInput(i) => {
                let gi = *self
                    .script_group
                    .input_indices
                    .get(*i as usize)
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))?;
                let cell = self
                    .rtx
                    .resolved_inputs
                    .get(gi)
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))?;
                self.data_loader.load_cell_data(cell).ok_or_else(|| {
                    Error::Unexpected(format!("Loading input cell #{}'s data failed!", gi))
                })
            }
            DataPieceId::GroupOutput(i) => {
                let gi = *self
                    .script_group
                    .output_indices
                    .get(*i as usize)
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))?;
                self.rtx
                    .transaction
                    .outputs_data()
                    .get(gi)
                    .map(|data| data.raw_data())
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))
            }
            DataPieceId::Witness(i) => self
                .rtx
                .transaction
                .witnesses()
                .get(*i as usize)
                .map(|data| data.raw_data())
                .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string())),
            DataPieceId::WitnessGroupInput(i) => {
                let gi = *self
                    .script_group
                    .input_indices
                    .get(*i as usize)
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))?;
                self.rtx
                    .transaction
                    .witnesses()
                    .get(gi)
                    .map(|data| data.raw_data())
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))
            }
            DataPieceId::WitnessGroupOutput(i) => {
                let gi = *self
                    .script_group
                    .output_indices
                    .get(*i as usize)
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))?;
                self.rtx
                    .transaction
                    .witnesses()
                    .get(gi)
                    .map(|data| data.raw_data())
                    .ok_or_else(|| Error::External("INDEX_OUT_OF_BOUND".to_string()))
            }
        }
        .map(|data| {
            let offset = std::cmp::min(offset as usize, data.len());
            let full_length = data.len() - offset;
            let slice_length = if length > 0 {
                std::cmp::min(full_length, length as usize)
            } else {
                full_length
            };
            (
                data.slice(offset..offset + slice_length),
                full_length as u64,
            )
        })
    }
}

#[derive(Clone)]
pub enum RunMode {
    LimitCycles(Cycle),
    Pause(Pause),
}
