use crate::ScriptError;
use ckb_error::Error;
use ckb_types::{
    core::{Cycle, ScriptHashType},
    packed::{Byte32, Script},
};
use ckb_vm::{
    machine::{VERSION0, VERSION1},
    snapshot::{make_snapshot, Snapshot},
    SupportMachine, ISA_B, ISA_IMC, ISA_MOP,
};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fmt;

#[cfg(has_asm)]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};

#[cfg(not(has_asm))]
use ckb_vm::{DefaultCoreMachine, SparseMemory, TraceMachine, WXorXMemory};

/// The type of CKB-VM ISA.
pub type VmIsa = u8;
/// /// The type of CKB-VM version.
pub type VmVersion = u32;

#[cfg(has_asm)]
pub(crate) type CoreMachineType = AsmCoreMachine;
#[cfg(not(has_asm))]
pub(crate) type CoreMachineType = DefaultCoreMachine<u64, WXorXMemory<SparseMemory<u64>>>;

/// The type of core VM machine when uses ASM.
#[cfg(has_asm)]
pub type CoreMachine = Box<AsmCoreMachine>;
/// The type of core VM machine when doesn't use ASM.
#[cfg(not(has_asm))]
pub type CoreMachine = DefaultCoreMachine<u64, WXorXMemory<SparseMemory<u64>>>;

/// The version of CKB Script Verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScriptVersion {
    /// CKB VM 0 with Syscall version 1.
    V0 = 0,
    /// CKB VM 1 with Syscall version 1 and version 2.
    V1 = 1,
}

impl ScriptVersion {
    /// Returns the latest version.
    pub const fn latest() -> Self {
        Self::V1
    }

    /// Returns the ISA set of CKB VM in current script version.
    pub fn vm_isa(self) -> VmIsa {
        match self {
            Self::V0 => ISA_IMC,
            Self::V1 => ISA_IMC | ISA_B | ISA_MOP,
        }
    }

    /// Returns the version of CKB VM in current script version.
    pub fn vm_version(self) -> VmVersion {
        match self {
            Self::V0 => VERSION0,
            Self::V1 => VERSION1,
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
pub(crate) type Machine<'a> = AsmMachine<'a>;
#[cfg(not(has_asm))]
pub(crate) type Machine<'a> = TraceMachine<'a, CoreMachine>;

pub struct ResumableMachine<'a> {
    pub(crate) machine: Machine<'a>,
    pub(crate) program_loaded: bool,
}

impl<'a> ResumableMachine<'a> {
    pub(crate) fn new(machine: Machine<'a>, program_loaded: bool) -> Self {
        ResumableMachine {
            machine,
            program_loaded,
        }
    }

    pub(crate) fn cycles(&self) -> Cycle {
        self.machine.machine.cycles()
    }

    #[cfg(test)]
    pub(crate) fn set_cycles(&mut self, cycles: Cycle) {
        self.machine.machine.set_cycles(cycles)
    }

    pub(crate) fn set_max_cycles(&mut self, cycles: Cycle) {
        set_vm_max_cycles(&mut self.machine, cycles)
    }

    pub fn program_loaded(&self) -> bool {
        self.program_loaded
    }
}

#[cfg(has_asm)]
pub(crate) fn set_vm_max_cycles(vm: &mut Machine<'_>, cycles: Cycle) {
    vm.set_max_cycles(cycles)
}

#[cfg(not(has_asm))]
pub(crate) fn set_vm_max_cycles(vm: &mut Machine<'_>, cycles: Cycle) {
    vm.machine.inner_mut().set_max_cycles(cycles)
}

/// A script group is defined as scripts that share the same hash.
///
/// A script group will only be executed once per transaction, the
/// script itself should check against all inputs/outputs in its group
/// if needed.
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
    /// current suspended script
    pub current: (ScriptGroupType, Byte32),
    /// remain script groups to verify
    pub remain: Vec<(ScriptGroupType, Byte32)>,
    /// vm snapshot
    pub snap: Option<Snapshot>,
    /// current consumed cycle
    pub current_cycles: Cycle,
    /// limit cycles when snapshot create
    pub limit_cycles: Cycle,
}

/// Struct specifies which script has verified so far.
/// State lifetime bound with vm machine.
pub struct TransactionState<'a> {
    /// current suspended script
    pub current: (ScriptGroupType, Byte32),
    /// remain script groups to verify
    pub remain: Vec<(ScriptGroupType, Byte32)>,
    /// vm state
    pub vm: ResumableMachine<'a>,
    /// current consumed cycle
    pub current_cycles: Cycle,
    /// limit cycles
    pub limit_cycles: Cycle,
}

impl TransactionState<'_> {
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

impl TryFrom<TransactionState<'_>> for TransactionSnapshot {
    type Error = Error;

    fn try_from(state: TransactionState<'_>) -> Result<Self, Self::Error> {
        let TransactionState {
            current,
            remain,
            mut vm,
            current_cycles,
            limit_cycles,
        } = state;

        // we should not capture snapshot if load program failed by exceeded cycles
        let snap =
            if vm.program_loaded {
                Some(make_snapshot(&mut vm.machine.machine).map_err(|e| {
                    ScriptError::VMInternalError(format!("{:?}", e)).unknown_source()
                })?)
            } else {
                None
            };

        Ok(TransactionSnapshot {
            current,
            remain,
            snap,
            current_cycles,
            limit_cycles,
        })
    }
}

/// Enum represent resumable verify result
#[derive(Debug)]
pub enum VerifyResult<'a> {
    /// Completed total cycles
    Completed(Cycle),
    /// Suspended state
    Suspended(TransactionState<'a>),
}

impl std::fmt::Debug for TransactionSnapshot {
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("TransactionSnapshot")
            .field("current", &self.current)
            .field("remain", &self.remain)
            .field("current_cycles", &self.current_cycles)
            .field("limit_cycles", &self.limit_cycles)
            .finish()
    }
}

impl std::fmt::Debug for TransactionState<'_> {
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("TransactionState")
            .field("current", &self.current)
            .field("remain", &self.remain)
            .field("current_cycles", &self.current_cycles)
            .field("limit_cycles", &self.limit_cycles)
            .finish()
    }
}
