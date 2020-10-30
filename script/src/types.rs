use ckb_types::packed::Script;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A script group is defined as scripts that share the same hash.
///
/// A script group will only be executed once per transaction, the
/// script itself should check against all inputs/outputs in its group
/// if needed.
pub struct ScriptGroup {
    /// TODO(doc): @doitian
    pub script: Script,
    /// TODO(doc): @doitian
    pub group_type: ScriptGroupType,
    /// TODO(doc): @doitian
    pub input_indices: Vec<usize>,
    /// TODO(doc): @doitian
    pub output_indices: Vec<usize>,
}

impl ScriptGroup {
    /// TODO(doc): @doitian
    pub fn new(script: &Script, group_type: ScriptGroupType) -> Self {
        Self {
            group_type,
            script: script.to_owned(),
            input_indices: vec![],
            output_indices: vec![],
        }
    }

    /// TODO(doc): @doitian
    pub fn from_lock_script(script: &Script) -> Self {
        Self::new(script, ScriptGroupType::Lock)
    }

    /// TODO(doc): @doitian
    pub fn from_type_script(script: &Script) -> Self {
        Self::new(script, ScriptGroupType::Type)
    }
}

/// TODO(doc): @doitian
#[derive(Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGroupType {
    /// TODO(doc): @doitian
    Lock,
    /// TODO(doc): @doitian
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
