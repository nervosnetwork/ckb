use crate::{error::ScriptError, verify_env::TxVerifyEnv};
use ckb_chain_spec::consensus::Consensus;
use ckb_types::{
    core::{
        cell::{CellMeta, ResolvedTransaction},
        Cycle, ScriptHashType,
    },
    packed::{Byte32, CellOutput, OutPoint, Script},
    prelude::*,
};
use ckb_vm::{
    machine::{VERSION0, VERSION1, VERSION2},
    ISA_B, ISA_IMC, ISA_MOP,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

#[cfg(has_asm)]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};

#[cfg(not(has_asm))]
use ckb_vm::{DefaultCoreMachine, TraceMachine, WXorXMemory};

use ckb_traits::CellDataProvider;
use ckb_vm::snapshot2::Snapshot2Context;

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

pub(crate) type DebugPrinter = Arc<dyn Fn(&Byte32, &str) + Send + Sync>;

pub struct DebugContext {
    pub debug_printer: DebugPrinter,
    #[cfg(test)]
    pub skip_pause: Arc<std::sync::atomic::AtomicBool>,
}

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
            Self::V2 => ISA_IMC | ISA_B | ISA_MOP,
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
#[derive(Clone, Debug)]
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

/// The methods included here are defected in a way: all construction
/// methods here create ScriptGroup without any `input_indices` or
/// `output_indices` filled. One has to manually fill them later(or forgot
/// about this).
/// As a result, we are marking them as crate-only methods for now. This
/// forces users to one of the following 2 solutions:
/// * Call `groups()` on `TxData` so they can fetch `ScriptGroup` data with
///   all correct data filled.
/// * Manually construct the struct where they have to think what shall be
///   used for `input_indices` and `output_indices`.
impl ScriptGroup {
    /// Creates a new script group struct.
    pub(crate) fn new(script: &Script, group_type: ScriptGroupType) -> Self {
        Self {
            group_type,
            script: script.to_owned(),
            input_indices: vec![],
            output_indices: vec![],
        }
    }

    /// Creates a lock script group.
    pub(crate) fn from_lock_script(script: &Script) -> Self {
        Self::new(script, ScriptGroupType::Lock)
    }

    /// Creates a type script group.
    pub(crate) fn from_type_script(script: &Script) -> Self {
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
/// State is lifetime free, but capture snapshot need heavy memory copy
#[derive(Clone)]
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

/// Enum represent resumable verify result
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum VerifyResult {
    /// Completed total cycles
    Completed(Cycle),
    /// Suspended state
    Suspended(TransactionState),
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
pub struct DataLocation {
    pub data_piece_id: DataPieceId,
    pub offset: u64,
    pub length: u64,
}

#[derive(Clone, Debug)]
pub struct ExecV2Args {
    pub location: DataLocation,
    pub argc: u64,
    pub argv: u64,
}

#[derive(Clone, Debug)]
pub struct SpawnArgs {
    pub location: DataLocation,
    pub argc: u64,
    pub argv: u64,
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
    ExecV2(VmId, ExecV2Args),
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

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DataGuard {
    NotLoaded(OutPoint),
    Loaded(Bytes),
}

/// LazyData wrapper make sure not-loaded data will be loaded only after one access
#[derive(Debug, Clone)]
pub struct LazyData(Arc<RwLock<DataGuard>>);

impl LazyData {
    fn from_cell_meta(cell_meta: &CellMeta) -> LazyData {
        match &cell_meta.mem_cell_data {
            Some(data) => LazyData(Arc::new(RwLock::new(DataGuard::Loaded(data.to_owned())))),
            None => LazyData(Arc::new(RwLock::new(DataGuard::NotLoaded(
                cell_meta.out_point.clone(),
            )))),
        }
    }

    fn access<DL: CellDataProvider>(&self, data_loader: &DL) -> Result<Bytes, ScriptError> {
        let guard = self
            .0
            .read()
            .map_err(|_| ScriptError::Other("RwLock poisoned".into()))?
            .to_owned();
        match guard {
            DataGuard::NotLoaded(out_point) => {
                let data = data_loader
                    .get_cell_data(&out_point)
                    .ok_or(ScriptError::Other("cell data not found".into()))?;
                let mut write_guard = self
                    .0
                    .write()
                    .map_err(|_| ScriptError::Other("RwLock poisoned".into()))?;
                *write_guard = DataGuard::Loaded(data.clone());
                Ok(data)
            }
            DataGuard::Loaded(bytes) => Ok(bytes),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Binaries {
    Unique(Byte32, usize, LazyData),
    Duplicate(Byte32, usize, LazyData),
    Multiple,
}

impl Binaries {
    fn new(data_hash: Byte32, dep_index: usize, data: LazyData) -> Self {
        Self::Unique(data_hash, dep_index, data)
    }

    fn merge(&mut self, data_hash: &Byte32) {
        match self {
            Self::Unique(ref hash, dep_index, data)
            | Self::Duplicate(ref hash, dep_index, data) => {
                if hash != data_hash {
                    *self = Self::Multiple;
                } else {
                    *self = Self::Duplicate(hash.to_owned(), *dep_index, data.to_owned());
                }
            }
            Self::Multiple => {}
        }
    }
}

/// Immutable context data at transaction level
#[derive(Clone, Debug)]
pub struct TxData<DL> {
    /// ResolvedTransaction.
    pub rtx: Arc<ResolvedTransaction>,
    /// Data loader.
    pub data_loader: DL,
    /// Chain consensus parameters
    pub consensus: Arc<Consensus>,
    /// Transaction verification environment
    pub tx_env: Arc<TxVerifyEnv>,

    /// Potential binaries in current transaction indexed by data hash
    pub binaries_by_data_hash: HashMap<Byte32, (usize, LazyData)>,
    /// Potential binaries in current transaction indexed by type script hash
    pub binaries_by_type_hash: HashMap<Byte32, Binaries>,
    /// Lock script groups, orders here are important
    pub lock_groups: BTreeMap<Byte32, ScriptGroup>,
    /// Type script groups, orders here are important
    pub type_groups: BTreeMap<Byte32, ScriptGroup>,
    /// Output cells in current transaction reorganized in CellMeta format
    pub outputs: Vec<CellMeta>,
}

impl<DL> TxData<DL>
where
    DL: CellDataProvider,
{
    /// Creates a new TxData structure
    pub fn new(
        rtx: Arc<ResolvedTransaction>,
        data_loader: DL,
        consensus: Arc<Consensus>,
        tx_env: Arc<TxVerifyEnv>,
    ) -> Self {
        let tx_hash = rtx.transaction.hash();
        let resolved_cell_deps = &rtx.resolved_cell_deps;
        let resolved_inputs = &rtx.resolved_inputs;
        let outputs = rtx
            .transaction
            .outputs_with_data_iter()
            .enumerate()
            .map(|(index, (cell_output, data))| {
                let out_point = OutPoint::new_builder()
                    .tx_hash(tx_hash.clone())
                    .index(index.pack())
                    .build();
                let data_hash = CellOutput::calc_data_hash(&data);
                CellMeta {
                    cell_output,
                    out_point,
                    transaction_info: None,
                    data_bytes: data.len() as u64,
                    mem_cell_data: Some(data),
                    mem_cell_data_hash: Some(data_hash),
                }
            })
            .collect();

        let mut binaries_by_data_hash: HashMap<Byte32, (usize, LazyData)> = HashMap::default();
        let mut binaries_by_type_hash: HashMap<Byte32, Binaries> = HashMap::default();
        for (i, cell_meta) in resolved_cell_deps.iter().enumerate() {
            let data_hash = data_loader
                .load_cell_data_hash(cell_meta)
                .expect("cell data hash");
            let lazy = LazyData::from_cell_meta(cell_meta);
            binaries_by_data_hash.insert(data_hash.to_owned(), (i, lazy.to_owned()));

            if let Some(t) = &cell_meta.cell_output.type_().to_opt() {
                binaries_by_type_hash
                    .entry(t.calc_script_hash())
                    .and_modify(|bin| bin.merge(&data_hash))
                    .or_insert_with(|| Binaries::new(data_hash.to_owned(), i, lazy.to_owned()));
            }
        }

        let mut lock_groups = BTreeMap::default();
        let mut type_groups = BTreeMap::default();
        for (i, cell_meta) in resolved_inputs.iter().enumerate() {
            // here we are only pre-processing the data, verify method validates
            // each input has correct script setup.
            let output = &cell_meta.cell_output;
            let lock_group_entry = lock_groups
                .entry(output.calc_lock_hash())
                .or_insert_with(|| ScriptGroup::from_lock_script(&output.lock()));
            lock_group_entry.input_indices.push(i);
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(t));
                type_group_entry.input_indices.push(i);
            }
        }
        for (i, output) in rtx.transaction.outputs().into_iter().enumerate() {
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(t));
                type_group_entry.output_indices.push(i);
            }
        }

        Self {
            rtx,
            data_loader,
            consensus,
            tx_env,
            binaries_by_data_hash,
            binaries_by_type_hash,
            lock_groups,
            type_groups,
            outputs,
        }
    }

    #[inline]
    /// Extracts actual script binary either in dep cells.
    pub fn extract_script(&self, script: &Script) -> Result<Bytes, ScriptError> {
        let (lazy, _) = self.extract_script_and_dep_index(script)?;
        lazy.access(&self.data_loader)
    }
}

impl<DL> TxData<DL> {
    #[inline]
    /// Extracts the index of the script binary in dep cells
    pub fn extract_referenced_dep_index(&self, script: &Script) -> Result<usize, ScriptError> {
        let (_, dep_index) = self.extract_script_and_dep_index(script)?;
        Ok(*dep_index)
    }

    fn extract_script_and_dep_index(
        &self,
        script: &Script,
    ) -> Result<(&LazyData, &usize), ScriptError> {
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data | ScriptHashType::Data1 | ScriptHashType::Data2 => {
                if let Some((dep_index, lazy)) = self.binaries_by_data_hash.get(&script.code_hash())
                {
                    Ok((lazy, dep_index))
                } else {
                    Err(ScriptError::ScriptNotFound(script.code_hash()))
                }
            }
            ScriptHashType::Type => {
                if let Some(ref bin) = self.binaries_by_type_hash.get(&script.code_hash()) {
                    match bin {
                        Binaries::Unique(_, dep_index, ref lazy) => Ok((lazy, dep_index)),
                        Binaries::Duplicate(_, dep_index, ref lazy) => Ok((lazy, dep_index)),
                        Binaries::Multiple => Err(ScriptError::MultipleMatches),
                    }
                } else {
                    Err(ScriptError::ScriptNotFound(script.code_hash()))
                }
            }
        }
    }

    #[inline]
    /// Calculates transaction hash
    pub fn tx_hash(&self) -> Byte32 {
        self.rtx.transaction.hash()
    }

    /// Finds the script group from cell deps.
    pub fn find_script_group(
        &self,
        script_group_type: ScriptGroupType,
        script_hash: &Byte32,
    ) -> Option<&ScriptGroup> {
        match script_group_type {
            ScriptGroupType::Lock => self.lock_groups.get(script_hash),
            ScriptGroupType::Type => self.type_groups.get(script_hash),
        }
    }

    fn is_vm_version_1_and_syscalls_2_enabled(&self) -> bool {
        // If the proposal window is allowed to prejudge on the vm version,
        // it will cause proposal tx to start a new vm in the blocks before hardfork,
        // destroying the assumption that the transaction execution only uses the old vm
        // before hardfork, leading to unexpected network splits.
        let epoch_number = self.tx_env.epoch_number_without_proposal_window();
        let hardfork_switch = self.consensus.hardfork_switch();
        hardfork_switch
            .ckb2021
            .is_vm_version_1_and_syscalls_2_enabled(epoch_number)
    }

    fn is_vm_version_2_and_syscalls_3_enabled(&self) -> bool {
        // If the proposal window is allowed to prejudge on the vm version,
        // it will cause proposal tx to start a new vm in the blocks before hardfork,
        // destroying the assumption that the transaction execution only uses the old vm
        // before hardfork, leading to unexpected network splits.
        let epoch_number = self.tx_env.epoch_number_without_proposal_window();
        let hardfork_switch = self.consensus.hardfork_switch();
        hardfork_switch
            .ckb2023
            .is_vm_version_2_and_syscalls_3_enabled(epoch_number)
    }

    /// Returns the version of the machine based on the script and the consensus rules.
    pub fn select_version(&self, script: &Script) -> Result<ScriptVersion, ScriptError> {
        let is_vm_version_2_and_syscalls_3_enabled = self.is_vm_version_2_and_syscalls_3_enabled();
        let is_vm_version_1_and_syscalls_2_enabled = self.is_vm_version_1_and_syscalls_2_enabled();
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data => Ok(ScriptVersion::V0),
            ScriptHashType::Data1 => {
                if is_vm_version_1_and_syscalls_2_enabled {
                    Ok(ScriptVersion::V1)
                } else {
                    Err(ScriptError::InvalidVmVersion(1))
                }
            }
            ScriptHashType::Data2 => {
                if is_vm_version_2_and_syscalls_3_enabled {
                    Ok(ScriptVersion::V2)
                } else {
                    Err(ScriptError::InvalidVmVersion(2))
                }
            }
            ScriptHashType::Type => {
                if is_vm_version_2_and_syscalls_3_enabled {
                    Ok(ScriptVersion::V2)
                } else if is_vm_version_1_and_syscalls_2_enabled {
                    Ok(ScriptVersion::V1)
                } else {
                    Ok(ScriptVersion::V0)
                }
            }
        }
    }

    /// Returns all script groups.
    pub fn groups(&self) -> impl Iterator<Item = (&'_ Byte32, &'_ ScriptGroup)> {
        self.lock_groups.iter().chain(self.type_groups.iter())
    }

    /// Returns all script groups with type.
    pub fn groups_with_type(
        &self,
    ) -> impl Iterator<Item = (ScriptGroupType, &'_ Byte32, &'_ ScriptGroup)> {
        self.lock_groups
            .iter()
            .map(|(hash, group)| (ScriptGroupType::Lock, hash, group))
            .chain(
                self.type_groups
                    .iter()
                    .map(|(hash, group)| (ScriptGroupType::Type, hash, group)),
            )
    }
}

/// Immutable context data at script group level
#[derive(Clone, Debug)]
pub struct SgData<DL> {
    /// Transaction level data
    pub tx_data: Arc<TxData<DL>>,

    /// Currently executed script version
    pub script_version: ScriptVersion,
    /// Currently executed script group
    pub script_group: ScriptGroup,
    /// DataPieceId for the root program
    pub program_data_piece_id: DataPieceId,
}

impl<DL> SgData<DL> {
    pub fn new(tx_data: &Arc<TxData<DL>>, script_group: &ScriptGroup) -> Result<Self, ScriptError> {
        let script_version = tx_data.select_version(&script_group.script)?;
        let dep_index = tx_data
            .extract_referenced_dep_index(&script_group.script)?
            .try_into()
            .map_err(|_| ScriptError::Other("u32 overflow".to_string()))?;
        Ok(Self {
            tx_data: Arc::clone(tx_data),
            script_version,
            script_group: script_group.clone(),
            program_data_piece_id: DataPieceId::CellDep(dep_index),
        })
    }
}

impl<DL> DataSource<DataPieceId> for Arc<SgData<DL>>
where
    DL: CellDataProvider,
{
    fn load_data(&self, id: &DataPieceId, offset: u64, length: u64) -> Option<(Bytes, u64)> {
        match id {
            DataPieceId::Input(i) => self
                .tx_data
                .rtx
                .resolved_inputs
                .get(*i as usize)
                .and_then(|cell| self.tx_data.data_loader.load_cell_data(cell)),
            DataPieceId::Output(i) => self
                .tx_data
                .rtx
                .transaction
                .outputs_data()
                .get(*i as usize)
                .map(|data| data.raw_data()),
            DataPieceId::CellDep(i) => self
                .tx_data
                .rtx
                .resolved_cell_deps
                .get(*i as usize)
                .and_then(|cell| self.tx_data.data_loader.load_cell_data(cell)),
            DataPieceId::GroupInput(i) => self
                .script_group
                .input_indices
                .get(*i as usize)
                .and_then(|gi| self.tx_data.rtx.resolved_inputs.get(*gi))
                .and_then(|cell| self.tx_data.data_loader.load_cell_data(cell)),
            DataPieceId::GroupOutput(i) => self
                .script_group
                .output_indices
                .get(*i as usize)
                .and_then(|gi| self.tx_data.rtx.transaction.outputs_data().get(*gi))
                .map(|data| data.raw_data()),
            DataPieceId::Witness(i) => self
                .tx_data
                .rtx
                .transaction
                .witnesses()
                .get(*i as usize)
                .map(|data| data.raw_data()),
            DataPieceId::WitnessGroupInput(i) => self
                .script_group
                .input_indices
                .get(*i as usize)
                .and_then(|gi| self.tx_data.rtx.transaction.witnesses().get(*gi))
                .map(|data| data.raw_data()),
            DataPieceId::WitnessGroupOutput(i) => self
                .script_group
                .output_indices
                .get(*i as usize)
                .and_then(|gi| self.tx_data.rtx.transaction.witnesses().get(*gi))
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

/// Immutable context data at virtual machine level
#[derive(Clone, Debug)]
pub struct VmData<DL> {
    /// Script group level data
    pub sg_data: Arc<SgData<DL>>,

    /// Currently executed virtual machine ID
    pub vm_id: VmId,
}

impl<DL> VmData<DL> {
    pub fn rtx(&self) -> &ResolvedTransaction {
        &self.sg_data.tx_data.rtx
    }

    pub fn data_loader(&self) -> &DL {
        &self.sg_data.tx_data.data_loader
    }

    pub fn group_inputs(&self) -> &[usize] {
        &self.sg_data.script_group.input_indices
    }

    pub fn group_outputs(&self) -> &[usize] {
        &self.sg_data.script_group.output_indices
    }

    pub fn outputs(&self) -> &[CellMeta] {
        &self.sg_data.tx_data.outputs
    }

    pub fn current_script_hash(&self) -> Byte32 {
        self.sg_data.script_group.script.calc_script_hash()
    }
}

impl<DL> DataSource<DataPieceId> for Arc<VmData<DL>>
where
    DL: CellDataProvider,
{
    fn load_data(&self, id: &DataPieceId, offset: u64, length: u64) -> Option<(Bytes, u64)> {
        self.sg_data.load_data(id, offset, length)
    }
}

/// Mutable data at virtual machine level
#[derive(Clone)]
pub struct VmContext<DL>
where
    DL: CellDataProvider,
{
    pub(crate) base_cycles: Arc<Mutex<u64>>,
    /// A mutable reference to scheduler's message box
    pub(crate) message_box: Arc<Mutex<Vec<Message>>>,
    pub(crate) snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, Arc<VmData<DL>>>>>,
}

impl<DL> VmContext<DL>
where
    DL: CellDataProvider,
{
    /// Creates a new VM context. It is by design that parameters to this function
    /// are references. It is a reminder that the inputs are designed to be shared
    /// among different entities.
    pub fn new(vm_data: &Arc<VmData<DL>>, message_box: &Arc<Mutex<Vec<Message>>>) -> Self {
        Self {
            base_cycles: Arc::new(Mutex::new(0)),
            message_box: Arc::clone(message_box),
            snapshot2_context: Arc::new(Mutex::new(Snapshot2Context::new(Arc::clone(vm_data)))),
        }
    }

    pub fn set_base_cycles(&mut self, base_cycles: u64) {
        *self.base_cycles.lock().expect("lock") = base_cycles;
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
