use ckb_types::packed::Script;
use serde::{Deserialize, Serialize};

// A script group is defined as scripts that share the same hash.
// A script group will only be executed once per transaction, the
// script itself should check against all inputs/outputs in its group
// if needed.
pub struct ScriptGroup {
    pub script: Script,
    pub input_indices: Vec<usize>,
    pub output_indices: Vec<usize>,
}

impl ScriptGroup {
    pub fn new(script: &Script) -> Self {
        Self {
            script: script.to_owned(),
            input_indices: vec![],
            output_indices: vec![],
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGroupType {
    Lock,
    Type,
}
