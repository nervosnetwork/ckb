use ckb_types::packed::Script;
use serde::{Deserialize, Serialize};
use std::fmt;

// A script group is defined as scripts that share the same hash.
// A script group will only be executed once per transaction, the
// script itself should check against all inputs/outputs in its group
// if needed.
pub struct ScriptGroup {
    pub script: Script,
    pub group_type: ScriptGroupType,
    pub input_indices: Vec<usize>,
    pub output_indices: Vec<usize>,
}

impl ScriptGroup {
    pub fn new(script: &Script, group_type: ScriptGroupType) -> Self {
        Self {
            group_type,
            script: script.to_owned(),
            input_indices: vec![],
            output_indices: vec![],
        }
    }

    pub fn from_lock_script(script: &Script) -> Self {
        Self::new(script, ScriptGroupType::Lock)
    }

    pub fn from_type_script(script: &Script) -> Self {
        Self::new(script, ScriptGroupType::Type)
    }
}

#[derive(Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGroupType {
    Lock,
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
