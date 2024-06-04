use crate::types::{ScriptGroup, ScriptGroupType};
use ckb_error::{prelude::*, Error, ErrorKind};
use ckb_types::core::{Cycle, ScriptHashType};
use ckb_types::packed::{Byte32, Script};
use ckb_vm::Error as VMInternalError;
use std::{error::Error as StdError, fmt};

/// Script execution error.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ScriptError {
    /// The field code_hash in script can't be resolved
    #[error("ScriptNotFound: code_hash: {0}")]
    ScriptNotFound(Byte32),

    /// The script consumes too much cycles
    #[error("ExceededMaximumCycles: expect cycles <= {0}")]
    ExceededMaximumCycles(Cycle),

    /// Internal error cycles overflow
    #[error("CyclesOverflow: lhs {0} rhs {1}")]
    CyclesOverflow(Cycle, Cycle),

    /// `script.type_hash` hits multiple cells with different data
    #[error("MultipleMatches")]
    MultipleMatches,

    /// Non-zero exit code returns by script
    #[error("ValidationFailure: see error code {1} on page https://nervosnetwork.github.io/ckb-script-error-codes/{0}.html#{1}")]
    ValidationFailure(String, i8),

    /// Known bugs are detected in transaction script outputs
    #[error("Known bugs encountered in output {1}: {0}")]
    EncounteredKnownBugs(String, usize),

    /// InvalidScriptHashType
    #[error("InvalidScriptHashType: {0}")]
    InvalidScriptHashType(String),

    /// InvalidVmVersion
    #[error("Invalid VM Version: {0}")]
    InvalidVmVersion(u8),

    /// Errors thrown by ckb-vm
    #[error("VM Internal Error: {0:?}")]
    VMInternalError(VMInternalError),

    /// Other errors raised in script execution process
    #[error("Other Error: {0}")]
    Other(String),
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
            TransactionScriptErrorSource::Inputs(n, field) => write!(f, "Inputs[{n}].{field}"),
            TransactionScriptErrorSource::Outputs(n, field) => {
                write!(f, "Outputs[{n}].{field}")
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

impl TransactionScriptError {
    /// Originating script for the generated error
    pub fn originating_script(&self) -> &TransactionScriptErrorSource {
        &self.source
    }

    /// Actual error generated when verifying script
    pub fn script_error(&self) -> &ScriptError {
        &self.cause
    }
}

/// It is a deliberate choice here to implement StdError directly, instead of
/// implementing thiserror::Error on TransactionScriptError. This way, calling
/// root_cause() on ckb_error::Error would return TransactionScriptError structure,
/// providing us enough information to inspect on all kinds of errors generated when
/// verifying a script.
///
/// This also means calling source() or cause() from std::error::Error on
/// TransactionScriptError would return None values. One is expected to cast
/// a value of `std::error::Error` type(possibly returned from root_cause) into
/// concrete TransactionScriptError type, then use the defined methods to fetch
/// originating script, as well as the actual script error. See the unit test defined
/// at the end of this file for an example.
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
    /// Creates a script error originated the script and its exit code.
    pub fn validation_failure(script: &Script, exit_code: i8) -> ScriptError {
        let url_path = match ScriptHashType::try_from(script.hash_type()).expect("checked data") {
            ScriptHashType::Data | ScriptHashType::Data1 | ScriptHashType::Data2 => {
                format!("by-data-hash/{:x}", script.code_hash())
            }
            ScriptHashType::Type => {
                format!("by-type-hash/{:x}", script.code_hash())
            }
        };

        ScriptError::ValidationFailure(url_path, exit_code)
    }

    ///  Creates a script error originated from the script group.
    pub fn source(self, script_group: &ScriptGroup) -> TransactionScriptError {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downcast_error_to_vm_error() {
        let vm_error = VMInternalError::ElfParseError("Foo bar baz".to_string());
        let script_error = ScriptError::VMInternalError(vm_error.clone());
        let error: Error = script_error.output_type_script(177).into();

        let recovered_transaction_error: TransactionScriptError = error
            .root_cause()
            .downcast_ref()
            .cloned()
            .expect("downcasting transaction error");
        assert_eq!(
            recovered_transaction_error.originating_script(),
            &TransactionScriptErrorSource::Outputs(177, ScriptGroupType::Type),
        );

        if let ScriptError::VMInternalError(recovered_vm_error) =
            recovered_transaction_error.script_error()
        {
            assert_eq!(recovered_vm_error, &vm_error);
        } else {
            panic!(
                "Invalid script type: {}",
                recovered_transaction_error.script_error()
            );
        }
    }
}
