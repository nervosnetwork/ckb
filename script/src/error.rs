use crate::types::ScriptGroup;
use ckb_error::{Error, ErrorKind};
use ckb_types::core::Cycle;
use failure::Fail;
use std::fmt;

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
    #[fail(display = "ValidationFailure({})", _0)]
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
    Inputs(usize),
    Outputs(usize),
    Unknown,
}

impl TransactionScriptErrorSource {
    fn from_script_group(script_group: &ScriptGroup) -> Self {
        if let Some(n) = script_group.input_indices.first() {
            TransactionScriptErrorSource::Inputs(*n)
        } else {
            if let Some(n) = script_group.output_indices.first() {
                TransactionScriptErrorSource::Outputs(*n)
            } else {
                TransactionScriptErrorSource::Unknown
            }
        }
    }
}

impl fmt::Display for TransactionScriptErrorSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TransactionScriptErrorSource::Inputs(n) => write!(f, "Inputs[{}]", n),
            TransactionScriptErrorSource::Outputs(n) => write!(f, "Outputs[{}]", n),
            TransactionScriptErrorSource::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub struct TransactionScriptError {
    cause: ScriptError,
    source: TransactionScriptErrorSource,
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

    pub fn source_input(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Inputs(index),
            cause: self,
        }
    }

    pub fn source_output(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Outputs(index),
            cause: self,
        }
    }
}

impl From<TransactionScriptError> for Error {
    fn from(error: TransactionScriptError) -> Self {
        error.context(ErrorKind::Script).into()
    }
}
