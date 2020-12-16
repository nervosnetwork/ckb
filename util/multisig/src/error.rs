//! Multi-signature error.

use ckb_error::{def_error_base_on_kind, prelude::*};

/// Multi-signature error kinds.
#[derive(Error, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ErrorKind {
    /// The count of signatures should be less than the count of private keys.
    #[error("The count of sigs should less than pks.")]
    SigCountOverflow,
    /// The count of signatures is less than the threshold.
    #[error("The count of sigs less than threshold.")]
    SigNotEnough,
    /// The verified signatures count is less than the threshold.
    #[error("Failed to meet threshold {threshold}, actual: {pass_sigs}.")]
    Threshold {
        /// The required count of valid signatures.
        threshold: usize,
        /// The actual count of valid signatures.
        pass_sigs: usize,
    },
}

def_error_base_on_kind!(Error, ErrorKind, "Multi-signature error.");
