use crate::ScriptError;
use ckb_error::Error;
use ckb_types::{
    core::{Cycle, ScriptHashType},
    packed::{Byte32, Script},
};
use ckb_vm::{
    machine::{VERSION0, VERSION1, VERSION2},
    snapshot::{make_snapshot, Snapshot},
    Error as VMInternalError, SupportMachine, ISA_A, ISA_B, ISA_IMC, ISA_MOP,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex};

#[cfg(has_asm)]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};

#[cfg(not(has_asm))]
use ckb_vm::{DefaultCoreMachine, TraceMachine, WXorXMemory};

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

#[cfg(has_asm)]
pub(crate) type Machine = AsmMachine;
#[cfg(not(has_asm))]
pub(crate) type Machine = TraceMachine<CoreMachine>;

/// Common data that would be shared amongst multiple VM instances.
/// One sample usage right now, is to capture suspended machines in
/// a chain of spawned machines.
#[derive(Default)]
pub struct MachineContext {
    pub(crate) suspended_machines: Vec<ResumableMachine>,
}

/// Data structure captured all environment data for a suspended machine
#[derive(Clone, Debug)]
pub enum ResumePoint {
    Initial,
    Spawn {
        callee_peak_memory: u64,
        callee_memory_limit: u64,
        content: Vec<u8>,
        content_length: u64,
        caller_exit_code_addr: u64,
        caller_content_addr: u64,
        caller_content_length_addr: u64,
    },
}

/// Data structure captured all the required data for a spawn syscall
#[derive(Clone, Debug)]
pub struct SpawnData {
    pub(crate) callee_peak_memory: u64,
    pub(crate) callee_memory_limit: u64,
    pub(crate) content: Arc<Mutex<Vec<u8>>>,
    pub(crate) content_length: u64,
    pub(crate) caller_exit_code_addr: u64,
    pub(crate) caller_content_addr: u64,
    pub(crate) caller_content_length_addr: u64,
}

impl TryFrom<&SpawnData> for ResumePoint {
    type Error = VMInternalError;

    fn try_from(value: &SpawnData) -> Result<Self, Self::Error> {
        let SpawnData {
            callee_peak_memory,
            callee_memory_limit,
            content,
            content_length,
            caller_exit_code_addr,
            caller_content_addr,
            caller_content_length_addr,
        } = value;
        Ok(ResumePoint::Spawn {
            callee_peak_memory: *callee_peak_memory,
            callee_memory_limit: *callee_memory_limit,
            content: content
                .lock()
                .map_err(|e| VMInternalError::Unexpected(format!("Lock error: {}", e)))?
                .clone(),
            content_length: *content_length,
            caller_exit_code_addr: *caller_exit_code_addr,
            caller_content_addr: *caller_content_addr,
            caller_content_length_addr: *caller_content_length_addr,
        })
    }
}

pub enum ResumableMachine {
    Initial(Machine),
    Spawn(Machine, SpawnData),
}

impl ResumableMachine {
    pub(crate) fn initial(machine: Machine) -> Self {
        ResumableMachine::Initial(machine)
    }

    pub(crate) fn spawn(machine: Machine, data: SpawnData) -> Self {
        ResumableMachine::Spawn(machine, data)
    }

    pub(crate) fn machine(&self) -> &Machine {
        match self {
            ResumableMachine::Initial(machine) => machine,
            ResumableMachine::Spawn(machine, _) => machine,
        }
    }

    pub(crate) fn machine_mut(&mut self) -> &mut Machine {
        match self {
            ResumableMachine::Initial(machine) => machine,
            ResumableMachine::Spawn(machine, _) => machine,
        }
    }

    pub(crate) fn cycles(&self) -> Cycle {
        self.machine().machine.cycles()
    }

    pub(crate) fn set_max_cycles(&mut self, cycles: Cycle) {
        set_vm_max_cycles(self.machine_mut(), cycles)
    }

    pub fn add_cycles(&mut self, cycles: Cycle) -> Result<(), VMInternalError> {
        self.machine_mut().machine.add_cycles(cycles)
    }

    pub fn run(&mut self) -> Result<i8, VMInternalError> {
        self.machine_mut().run()
    }
}

#[cfg(has_asm)]
pub(crate) fn set_vm_max_cycles(vm: &mut Machine, cycles: Cycle) {
    vm.set_max_cycles(cycles)
}

#[cfg(not(has_asm))]
pub(crate) fn set_vm_max_cycles(vm: &mut Machine, cycles: Cycle) {
    vm.machine.inner_mut().set_max_cycles(cycles)
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
    pub snaps: Vec<(Snapshot, Cycle, ResumePoint)>,
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
    /// vm states
    pub vms: Vec<ResumableMachine>,
    /// current consumed cycle
    pub current_cycles: Cycle,
    /// limit cycles
    pub limit_cycles: Cycle,
    /// machine context for the vms included in this state
    pub machine_context: Arc<Mutex<MachineContext>>,
}

impl TransactionState {
    /// Creates a new TransactionState struct
    pub fn new(
        vms: Vec<ResumableMachine>,
        machine_context: Arc<Mutex<MachineContext>>,
        current: usize,
        current_cycles: Cycle,
        limit_cycles: Cycle,
    ) -> Self {
        TransactionState {
            current,
            vms,
            machine_context,
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
            vms,
            current_cycles,
            limit_cycles,
            ..
        } = state;

        let mut snaps = Vec::with_capacity(vms.len());
        for mut vm in vms {
            let snapshot = make_snapshot(&mut vm.machine_mut().machine)
                .map_err(|e| ScriptError::VMInternalError(format!("{e:?}")).unknown_source())?;
            let cycles = vm.cycles();
            let resume_point = match vm {
                ResumableMachine::Initial(_) => ResumePoint::Initial,
                ResumableMachine::Spawn(_, data) => (&data)
                    .try_into()
                    .map_err(|e| ScriptError::VMInternalError(format!("{e:?}")).unknown_source())?,
            };
            snaps.push((snapshot, cycles, resume_point));
        }

        Ok(TransactionSnapshot {
            current,
            snaps,
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
