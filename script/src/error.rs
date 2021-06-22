use crate::types::{ScriptGroup, ScriptGroupType};
use ckb_error::{prelude::*, Error, ErrorKind};
use ckb_types::core::Cycle;
use std::{error::Error as StdError, fmt};

/// Script execution error.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ScriptError {
    /// The field code_hash in script is invalid
    #[error("InvalidCodeHash")]
    InvalidCodeHash,

    /// The script consumes too much cycles
    #[error("ExceededMaximumCycles: expect cycles <= {0}")]
    ExceededMaximumCycles(Cycle),

    /// `script.type_hash` hits multiple cells with different data
    #[error("MultipleMatches")]
    MultipleMatches,

    /// Non-zero exit code returns by script
    #[error("ValidationFailure({0}): the exit code is per script specific, for system scripts, please check https://github.com/nervosnetwork/ckb-system-scripts/wiki/Error-codes")]
    ValidationFailure(i8),

    /// Known bugs are detected in transaction script outputs
    #[error("Known bugs encountered in output {1}: {0}")]
    EncounteredKnownBugs(String, usize),

    /// InvalidScriptHashType
    #[error("InvalidScriptHashType: {0}")]
    InvalidScriptHashType(String),

    /// InvalidVmVersion
    #[error("Invalid vm version {0}")]
    InvalidVmVersion(u8),

    /// Known bugs are detected in transaction script outputs
    #[error("VM Internal Error: {0}")]
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

/// Script execution error with the error source information.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TransactionScriptError {
    source: TransactionScriptErrorSource,
    cause: ScriptError,
}

impl StdError for TransactionScriptError {}

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

    /// Creates a script error originated from the lock script of the input cell at the specific index.
    pub fn input_lock_script(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Inputs(index, ScriptGroupType::Lock),
            cause: self,
        }
    }

    /// Creates a script error originated from the type script of the input cell at the specific index.
    pub fn input_type_script(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Inputs(index, ScriptGroupType::Type),
            cause: self,
        }
    }

    /// Creates a script error originated from the type script of the output cell at the specific index.
    pub fn output_type_script(self, index: usize) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Outputs(index, ScriptGroupType::Type),
            cause: self,
        }
    }

    /// Creates a script error with unknown source, usually a internal error
    pub fn unknown_source(self) -> TransactionScriptError {
        TransactionScriptError {
            source: TransactionScriptErrorSource::Unknown,
            cause: self,
        }
    }
}

impl From<TransactionScriptError> for Error {
    fn from(error: TransactionScriptError) -> Self {
        ErrorKind::Script.because(error)
    }
}
