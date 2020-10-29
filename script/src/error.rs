use crate::types::{ScriptGroup, ScriptGroupType};
use ckb_error::{Error, ErrorKind};
use ckb_types::core::Cycle;
use failure::Fail;
use std::fmt;

/// TODO(doc): @doitian
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum ScriptError {
    /// The field code_hash in script is invalid
    #[fail(display = "InvalidCodeHash")]
    InvalidCodeHash,

    /// The script consumes too much cycles
    #[fail(display = "ExceededMaximumCycles: expect cycles <= {}", _0)]
    ExceededMaximumCycles(Cycle),

    /// `script.type_hash` hits multiple cells with different data
    #[fail(display = "MultipleMatches")]
    MultipleMatches,

    /// Non-zero exit code returns by script
    #[fail(
        display = "ValidationFailure({}): the exit code is per script specific, for system scripts, please check https://github.com/nervosnetwork/ckb-system-scripts/wiki/Error-codes",
        _0
    )]
    ValidationFailure(i8),

    /// Known bugs are detected in transaction script outputs
    #[fail(display = "Known bugs encountered in output {}: {}", _1, _0)]
    EncounteredKnownBugs(String, usize),

    /// Known bugs are detected in transaction script outputs
    #[fail(display = "VM Internal Error: {}", _0)]
    VMInternalError(String),
}

/// Locate the script using the first input index if possible, otherwise the first output index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionScriptErrorSource {
    Inputs(usize, ScriptGroupType),
    Outputs(usize, ScriptGroupType),
    Unknown,
}

impl TransactionScriptErrorSource {
    fn from_script_group(script_group: &ScriptGroup) -> Self {
        if let Some(n) = script_group.input_indices.first() {
            TransactionScriptErrorSource::Inputs(*n, script_group.group_type)
        } else if let Some(n) = script_group.output_indices.first() {
            TransactionScriptErrorSource::Outputs(*n, script_group.group_type)
        } else {
            TransactionScriptErrorSource::Unknown
        }
    }
}

impl fmt::Display for TransactionScriptErrorSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TransactionScriptErrorSource::Inputs(n, field) => write!(f, "Inputs[{}].{}", n, field),
            TransactionScriptErrorSource::Outputs(n, field) => {
                write!(f, "Outputs[{}].{}", n, field)
            }
            TransactionScriptErrorSource::Unknown => write!(f, "Unknown"),
        }
    }
}

/// TODO(doc): @doitian
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub struct TransactionScriptError {
    source: TransactionScriptErrorSource,
    cause: ScriptError,
}

impl fmt::Display for TransactionScriptError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "TransactionScriptError {{ source: {}, cause: {} }}",
            self.source, self.cause
        )
    }
}

impl ScriptError {
    pub(crate) fn source(self, script_group: &ScriptGroup) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::from_script_group(script_group),
            cause: self,
        }
    }

    /// TODO(doc): @doitian
    pub fn input_lock_script(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Inputs(index, ScriptGroupType::Lock),
            cause: self,
        }
    }

    /// TODO(doc): @doitian
    pub fn input_type_script(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Inputs(index, ScriptGroupType::Type),
            cause: self,
        }
    }

    /// TODO(doc): @doitian
    pub fn output_type_script(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Outputs(index, ScriptGroupType::Type),
            cause: self,
        }
    }
}

impl From<TransactionScriptError> for Error {
    fn from(error: TransactionScriptError) -> Self {
        error.context(ErrorKind::Script).into()
    }
}
