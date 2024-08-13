use ckb_error::Error;
use ckb_types::{
    core::{Cycle, ScriptHashType},
    packed::{Byte32, Script},
};
use ckb_vm::{
    machine::{VERSION0, VERSION1, VERSION2},
    ISA_A, ISA_B, ISA_IMC, ISA_MOP,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex};

#[cfg(has_asm)]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};

#[cfg(not(has_asm))]
use ckb_vm::{DefaultCoreMachine, TraceMachine, WXorXMemory};

use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::snapshot2::Snapshot2Context;

use ckb_types::core::cell::ResolvedTransaction;
use ckb_vm::{
    bytes::Bytes,
    machine::Pause,
    snapshot2::{DataSource, Snapshot2},
    RISCV_GENERAL_REGISTER_NUMBER,
};
use std::mem::size_of;

/// The type of CKB-VM ISA.
pub type VmIsa = u8;
/// /// The type of CKB-VM version.
pub type VmVersion = u32;

#[cfg(has_asm)]
pub(crate) type CoreMachineType = AsmCoreMachine;
#[cfg(all(not(has_asm), not(feature = "flatmemory")))]
pub(crate) type CoreMachineType = DefaultCoreMachine<u64, WXorXMemory<ckb_vm::SparseMemory<u64>>>;
#[cfg(all(not(has_asm), feature = "flatmemory"))]
pub(crate) type CoreMachineType = DefaultCoreMachine<u64, WXorXMemory<ckb_vm::FlatMemory<u64>>>;

/// The type of core VM machine when uses ASM.
#[cfg(has_asm)]
pub type CoreMachine = Box<AsmCoreMachine>;
/// The type of core VM machine when doesn't use ASM.
#[cfg(all(not(has_asm), not(feature = "flatmemory")))]
pub type CoreMachine = DefaultCoreMachine<u64, WXorXMemory<ckb_vm::SparseMemory<u64>>>;
#[cfg(all(not(has_asm), feature = "flatmemory"))]
pub type CoreMachine = DefaultCoreMachine<u64, WXorXMemory<ckb_vm::FlatMemory<u64>>>;

#[cfg(has_asm)]
pub(crate) type Machine = AsmMachine;
#[cfg(not(has_asm))]
pub(crate) type Machine = TraceMachine<CoreMachine>;

pub(crate) type Indices = Arc<Vec<usize>>;

pub(crate) type DebugPrinter = Arc<dyn Fn(&Byte32, &str) + Send + Sync>;

/// The version of CKB Script Verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScriptVersion {
    /// CKB VM 0 with Syscall version 1.
    V0 = 0,
    /// CKB VM 1 with Syscall version 1 and version 2.
    V1 = 1,
    /// CKB VM 2 with Syscall version 1, version 2 and version 3.
    V2 = 2,
}

impl ScriptVersion {
    /// Returns the latest version.
    pub const fn latest() -> Self {
        Self::V2
    }

    /// Returns the ISA set of CKB VM in current script version.
    pub fn vm_isa(self) -> VmIsa {
        match self {
            Self::V0 => ISA_IMC,
            Self::V1 => ISA_IMC | ISA_B | ISA_MOP,
            Self::V2 => ISA_IMC | ISA_A | ISA_B | ISA_MOP,
        }
    }

    /// Returns the version of CKB VM in current script version.
    pub fn vm_version(self) -> VmVersion {
        match self {
            Self::V0 => VERSION0,
            Self::V1 => VERSION1,
            Self::V2 => VERSION2,
        }
    }

    /// Returns the specific data script hash type.
    ///
    /// Returns:
    /// - `ScriptHashType::Data` for version 0;
    /// - `ScriptHashType::Data1` for version 1;
    pub fn data_hash_type(self) -> ScriptHashType {
        match self {
            Self::V0 => ScriptHashType::Data,
            Self::V1 => ScriptHashType::Data1,
            Self::V2 => ScriptHashType::Data2,
        }
    }

    /// Creates a CKB VM core machine without cycles limit.
    ///
    /// In fact, there is still a limit of `max_cycles` which is set to `2^64-1`.
    pub fn init_core_machine_without_limit(self) -> CoreMachine {
        self.init_core_machine(u64::MAX)
    }

    /// Creates a CKB VM core machine.
    pub fn init_core_machine(self, max_cycles: Cycle) -> CoreMachine {
        let isa = self.vm_isa();
        let version = self.vm_version();
        CoreMachineType::new(isa, version, max_cycles)
    }
}

/// A script group is defined as scripts that share the same hash.
///
/// A script group will only be executed once per transaction, the
/// script itself should check against all inputs/outputs in its group
/// if needed.
#[derive(Clone)]
pub struct ScriptGroup {
    /// The script.
    ///
    /// A script group is a group of input and output cells that share the same script.
    pub script: Script,
    /// The script group type.
    pub group_type: ScriptGroupType,
    /// Indices of input cells.
    pub input_indices: Vec<usize>,
    /// Indices of output cells.
    pub output_indices: Vec<usize>,
}

impl ScriptGroup {
    /// Creates a new script group struct.
    pub fn new(script: &Script, group_type: ScriptGroupType) -> Self {
        Self {
            group_type,
            script: script.to_owned(),
            input_indices: vec![],
            output_indices: vec![],
        }
    }

    /// Creates a lock script group.
    pub fn from_lock_script(script: &Script) -> Self {
        Self::new(script, ScriptGroupType::Lock)
    }

    /// Creates a type script group.
    pub fn from_type_script(script: &Script) -> Self {
        Self::new(script, ScriptGroupType::Type)
    }
}

/// The script group type.
///
/// A cell can have a lock script and an optional type script. Even they reference the same script,
/// lock script and type script will not be grouped together.
#[derive(Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGroupType {
    /// Lock script group.
    Lock,
    /// Type script group.
    Type,
}

impl fmt::Display for ScriptGroupType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ScriptGroupType::Lock => write!(f, "Lock"),
            ScriptGroupType::Type => write!(f, "Type"),
        }
    }
}

/// Struct specifies which script has verified so far.
/// Snapshot is lifetime free, but capture snapshot need heavy memory copy
pub struct TransactionSnapshot {
    /// current suspended script index
    pub current: usize,
    /// vm snapshots
    pub state: Option<FullSuspendedState>,
    /// current consumed cycle
    pub current_cycles: Cycle,
    /// limit cycles when snapshot create
    pub limit_cycles: Cycle,
}

/// Struct specifies which script has verified so far.
/// State lifetime bound with vm machine.
pub struct TransactionState {
    /// current suspended script index
    pub current: usize,
    /// vm scheduler suspend state
    pub state: Option<FullSuspendedState>,
    /// current consumed cycle
    pub current_cycles: Cycle,
    /// limit cycles
    pub limit_cycles: Cycle,
}

impl TransactionState {
    /// Creates a new TransactionState struct
    pub fn new(
        state: Option<FullSuspendedState>,
        current: usize,
        current_cycles: Cycle,
        limit_cycles: Cycle,
    ) -> Self {
        TransactionState {
            current,
            state,
            current_cycles,
            limit_cycles,
        }
    }

    /// Return next limit cycles according to max_cycles and step_cycles
    pub fn next_limit_cycles(&self, step_cycles: Cycle, max_cycles: Cycle) -> (Cycle, bool) {
        let remain = max_cycles - self.current_cycles;
        let next_limit = self.limit_cycles + step_cycles;

        if next_limit < remain {
            (next_limit, false)
        } else {
            (remain, true)
        }
    }
}

impl TransactionSnapshot {
    /// Return next limit cycles according to max_cycles and step_cycles
    pub fn next_limit_cycles(&self, step_cycles: Cycle, max_cycles: Cycle) -> (Cycle, bool) {
        let remain = max_cycles - self.current_cycles;
        let next_limit = self.limit_cycles + step_cycles;

        if next_limit < remain {
            (next_limit, false)
        } else {
            (remain, true)
        }
    }
}

impl TryFrom<TransactionState> for TransactionSnapshot {
    type Error = Error;

    fn try_from(state: TransactionState) -> Result<Self, Self::Error> {
        let TransactionState {
            current,
            state,
            current_cycles,
            limit_cycles,
            ..
        } = state;

        Ok(TransactionSnapshot {
            current,
            state,
            current_cycles,
            limit_cycles,
        })
    }
}

/// Enum represent resumable verify result
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum VerifyResult {
    /// Completed total cycles
    Completed(Cycle),
    /// Suspended state
    Suspended(TransactionState),
}

impl std::fmt::Debug for TransactionSnapshot {
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("TransactionSnapshot")
            .field("current", &self.current)
            .field("current_cycles", &self.current_cycles)
            .field("limit_cycles", &self.limit_cycles)
            .finish()
    }
}

impl std::fmt::Debug for TransactionState {
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("TransactionState")
            .field("current", &self.current)
            .field("current_cycles", &self.current_cycles)
            .field("limit_cycles", &self.limit_cycles)
            .finish()
    }
}

/// ChunkCommand is used to control the verification process to suspend or resume
#[derive(Eq, PartialEq, Clone, Debug)]
pub enum ChunkCommand {
    /// Suspend the verification process
    Suspend,
    /// Resume the verification process
    Resume,
    /// Stop the verification process
    Stop,
}

#[derive(Clone)]
pub struct MachineContext<
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
> {
    pub(crate) base_cycles: Arc<Mutex<u64>>,
    pub(crate) snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
}

impl<DL> MachineContext<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub fn new(tx_data: TxData<DL>) -> Self {
        Self {
            base_cycles: Arc::new(Mutex::new(0)),
            snapshot2_context: Arc::new(Mutex::new(Snapshot2Context::new(tx_data))),
        }
    }

    pub fn snapshot2_context(&self) -> &Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>> {
        &self.snapshot2_context
    }

    pub fn set_base_cycles(&mut self, base_cycles: u64) {
        *self.base_cycles.lock().expect("lock") = base_cycles;
    }
}

pub type VmId = u64;
pub const FIRST_VM_ID: VmId = 0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fd(pub(crate) u64);

pub const FIRST_FD_SLOT: u64 = 2;

impl Fd {
    pub fn create(slot: u64) -> (Fd, Fd, u64) {
        (Fd(slot), Fd(slot + 1), slot + 2)
    }

    pub fn other_fd(&self) -> Fd {
        Fd(self.0 ^ 0x1)
    }

    pub fn is_read(&self) -> bool {
        self.0 % 2 == 0
    }

    pub fn is_write(&self) -> bool {
        self.0 % 2 == 1
    }
}

/// VM is in waiting-to-read state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReadState {
    pub fd: Fd,
    pub length: u64,
    pub buffer_addr: u64,
    pub length_addr: u64,
}

/// VM is in waiting-to-write state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WriteState {
    pub fd: Fd,
    pub consumed: u64,
    pub length: u64,
    pub buffer_addr: u64,
    pub length_addr: u64,
}

/// VM State.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VmState {
    /// Runnable.
    Runnable,
    /// Terminated.
    Terminated,
    /// Wait.
    Wait {
        /// Target vm id.
        target_vm_id: VmId,
        /// Exit code addr.
        exit_code_addr: u64,
    },
    /// WaitForWrite.
    WaitForWrite(WriteState),
    /// WaitForRead.
    WaitForRead(ReadState),
}

#[derive(Clone, Debug)]
pub struct SpawnArgs {
    pub data_piece_id: DataPieceId,
    pub offset: u64,
    pub length: u64,
    pub argv: Vec<Bytes>,
    pub fds: Vec<Fd>,
    pub process_id_addr: u64,
}

#[derive(Clone, Debug)]
pub struct WaitArgs {
    pub target_id: VmId,
    pub exit_code_addr: u64,
}

#[derive(Clone, Debug)]
pub struct PipeArgs {
    pub fd1_addr: u64,
    pub fd2_addr: u64,
}

#[derive(Clone, Debug)]
pub struct FdArgs {
    pub fd: Fd,
    pub length: u64,
    pub buffer_addr: u64,
    pub length_addr: u64,
}

#[derive(Clone, Debug)]
pub enum Message {
    Spawn(VmId, SpawnArgs),
    Wait(VmId, WaitArgs),
    Pipe(VmId, PipeArgs),
    FdRead(VmId, FdArgs),
    FdWrite(VmId, FdArgs),
    InheritedFileDescriptor(VmId, FdArgs),
    Close(VmId, Fd),
}

/// A pointer to the data that is part of the transaction.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DataPieceId {
    /// Target program. Usually located in cell data.
    Program,
    /// The nth input cell data.
    Input(u32),
    /// The nth output data.
    Output(u32),
    /// The nth cell dep cell data.
    CellDep(u32),
    /// The nth group input cell data.
    GroupInput(u32),
    /// The nth group output data.
    GroupOutput(u32),
    /// The nth witness.
    Witness(u32),
    /// The nth witness group input.
    WitnessGroupInput(u32),
    /// The nth witness group output.
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
    pub total_cycles: Cycle,
    pub next_vm_id: VmId,
    pub next_fd_slot: u64,
    pub vms: Vec<(VmId, VmState, Snapshot2<DataPieceId>)>,
    pub fds: Vec<(Fd, VmId)>,
    pub inherited_fd: Vec<(VmId, Vec<Fd>)>,
    pub terminated_vms: Vec<(VmId, i8)>,
    pub instantiated_ids: Vec<VmId>,
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
            + (self.fds.len() * (size_of::<Fd>() + size_of::<VmId>()))) as u64
            + (self.inherited_fd.len() * (size_of::<Fd>())) as u64
            + (self.terminated_vms.len() * (size_of::<VmId>() + size_of::<i8>())) as u64
            + (self.instantiated_ids.len() * size_of::<VmId>()) as u64
    }
}

/// Context data for current running transaction & script
#[derive(Clone)]
pub struct TxData<DL> {
    /// ResolvedTransaction.
    pub rtx: Arc<ResolvedTransaction>,
    /// Data loader.
    pub data_loader: DL,
    /// Ideally one might not want to keep program here, since program is totally
    /// deducible from rtx + data_loader, however, for a demo here, program
    /// does help us save some extra coding.
    pub program: Bytes,
    /// The script group to which the current program belongs.
    pub script_group: Arc<ScriptGroup>,
}

impl<DL> DataSource<DataPieceId> for TxData<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    fn load_data(&self, id: &DataPieceId, offset: u64, length: u64) -> Option<(Bytes, u64)> {
        match id {
            DataPieceId::Program => {
                // This is just a shortcut so we don't have to copy over the logic in extract_script,
                // ideally you can also only define the rest 5, then figure out a way to convert
                // script group to the actual cell dep index.
                Some(self.program.clone())
            }
            DataPieceId::Input(i) => self
                .rtx
                .resolved_inputs
                .get(*i as usize)
                .and_then(|cell| self.data_loader.load_cell_data(cell)),
            DataPieceId::Output(i) => self
                .rtx
                .transaction
                .outputs_data()
                .get(*i as usize)
                .map(|data| data.raw_data()),
            DataPieceId::CellDep(i) => self
                .rtx
                .resolved_cell_deps
                .get(*i as usize)
                .and_then(|cell| self.data_loader.load_cell_data(cell)),
            DataPieceId::GroupInput(i) => self
                .script_group
                .input_indices
                .get(*i as usize)
                .and_then(|gi| self.rtx.resolved_inputs.get(*gi))
                .and_then(|cell| self.data_loader.load_cell_data(cell)),
            DataPieceId::GroupOutput(i) => self
                .script_group
                .output_indices
                .get(*i as usize)
                .and_then(|gi| self.rtx.transaction.outputs_data().get(*gi))
                .map(|data| data.raw_data()),
            DataPieceId::Witness(i) => self
                .rtx
                .transaction
                .witnesses()
                .get(*i as usize)
                .map(|data| data.raw_data()),
            DataPieceId::WitnessGroupInput(i) => self
                .script_group
                .input_indices
                .get(*i as usize)
                .and_then(|gi| self.rtx.transaction.witnesses().get(*gi))
                .map(|data| data.raw_data()),
            DataPieceId::WitnessGroupOutput(i) => self
                .script_group
                .output_indices
                .get(*i as usize)
                .and_then(|gi| self.rtx.transaction.witnesses().get(*gi))
                .map(|data| data.raw_data()),
        }
        .map(|data| {
            let offset = std::cmp::min(offset as usize, data.len());
            let full_length = data.len() - offset;
            let real_length = if length > 0 {
                std::cmp::min(full_length, length as usize)
            } else {
                full_length
            };
            (data.slice(offset..offset + real_length), full_length as u64)
        })
    }
}

/// The scheduler's running mode.
#[derive(Clone)]
pub enum RunMode {
    /// Continues running until cycles are exhausted.
    LimitCycles(Cycle),
    /// Continues running until a Pause signal is received.
    Pause(Pause),
}
