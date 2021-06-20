use crate::ScriptError;
use ckb_error::Error;
use ckb_types::{
    core::Cycle,
    packed::{Byte32, Script},
};
use ckb_vm::snapshot::{make_snapshot, Snapshot};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fmt;

#[cfg(has_asm)]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};

#[cfg(not(has_asm))]
use ckb_vm::{DefaultCoreMachine, SparseMemory, TraceMachine, WXorXMemory};

#[cfg(has_asm)]
pub(crate) type CoreMachineType = Box<AsmCoreMachine>;
#[cfg(not(has_asm))]
pub(crate) type CoreMachineType = DefaultCoreMachine<u64, WXorXMemory<SparseMemory<u64>>>;

#[cfg(has_asm)]
pub(crate) type Machine<'a> = AsmMachine<'a>;
#[cfg(not(has_asm))]
pub(crate) type Machine<'a> = TraceMachine<'a, CoreMachineType>;

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
    pub snap: Snapshot,
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
    pub vm: Machine<'a>,
    /// current consumed cycle
    pub current_cycles: Cycle,
    /// limit cycles
    pub limit_cycles: Cycle,
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

        let snap = make_snapshot(&mut vm.machine)
            .map_err(|e| ScriptError::VMInternalError(format!("{:?}", e)).unknown_source())?;

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
