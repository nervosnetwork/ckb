use ckb_types::{
    core::Cycle,
    packed::{Byte32, Script},
};
use ckb_vm::snapshot::Snapshot;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

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
/// Use this value when resuming a suspended verify.
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

/// Enum represent resumable verify result
#[derive(Clone, Debug)]
pub enum VerifyResult {
    /// Completed total cycles
    Completed(Cycle),
    /// Suspended snapshot
    Suspended(Arc<TransactionSnapshot>),
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
