//! The error types to unexpected out-points.

use crate::core::{Capacity, Version};
use crate::generated::packed::{Byte32, OutPoint};
use ckb_error::{impl_error_conversion_with_kind, prelude::*, Error, ErrorKind};
use derive_more::Display;

/// Errors due to the fact that the out-point rules are not respected.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The target cell was already dead.
    #[error("Dead({0:?})")]
    Dead(OutPoint),

    /// There are cells which is unknown to the canonical chain.
    #[error("Unknown({0:?})")]
    Unknown(OutPoint),

    /// There is an input out-point or dependency out-point which references a newer cell in the same block.
    #[error("OutOfOrder({0:?})")]
    OutOfOrder(OutPoint),

    /// There is a dependency out-point, which is [`DepGroup`], but its output-data is invalid format. The expected output-data format for [`DepGroup`] is [`OutPointVec`].
    ///
    /// [`DepGroup`]: ../enum.DepType.html#variant.DepGroup
    /// [`OutPointVec`]: ../../packed/struct.OutPointVec.html
    #[error("InvalidDepGroup({0:?})")]
    InvalidDepGroup(OutPoint),

    /// There is a dependency header that is unknown to the canonical chain.
    #[error("InvalidHeader({0})")]
    InvalidHeader(Byte32),

    /// Over max dep expansion limit.
    #[error("OverMaxDepExpansionLimit")]
    OverMaxDepExpansionLimit,
}

impl From<OutPointError> for Error {
    fn from(error: OutPointError) -> Self {
        ErrorKind::OutPoint.because(error)
    }
}

/// Enum represent transaction relate error source
#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum TransactionErrorSource {
    /// cell deps
    CellDeps,
    /// header deps
    HeaderDeps,
    /// inputs
    Inputs,
    /// outputs
    Outputs,
    /// outputs data
    OutputsData,
    /// witnesses
    Witnesses,
}

/// The error types to transactions.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum TransactionError {
    /// There is an erroneous output that its occupied capacity is greater than its capacity (`output.occupied_capacity() > output.capacity()`).
    #[error("InsufficientCellCapacity({inner}[{index}]): expected occupied capacity ({occupied_capacity:#x}) <= capacity ({capacity:#x})")]
    InsufficientCellCapacity {
        /// The transaction field that causes error.
        /// It should always be `TransactionErrorSource::Outputs.`
        inner: TransactionErrorSource,
        /// The index of that erroneous output.
        index: usize,
        /// The occupied capacity of that erroneous output.
        occupied_capacity: Capacity,
        /// The capacity of that erroneous output.
        capacity: Capacity,
    },

    /// The total capacity of outputs is less than the total capacity of inputs (`SUM([o.capacity for o in outputs]) > SUM([i.capacity for i in inputs]`).
    #[error("OutputsSumOverflow: expected outputs capacity ({outputs_sum:#x}) <= inputs capacity ({inputs_sum:#x})")]
    OutputsSumOverflow {
        /// The total capacity of inputs.
        inputs_sum: Capacity,
        /// The total capacity of outputs.
        outputs_sum: Capacity,
    },

    /// Either inputs or outputs of the transaction are empty (`inputs.is_empty() || outputs.is_empty()`).
    #[error("Empty({inner})")]
    Empty {
        /// The transaction field that causes the error.
        /// It should be `TransactionErrorSource::Inputs` or `TransactionErrorSource::Outputs`.
        inner: TransactionErrorSource,
    },

    /// There are duplicated [`CellDep`]s within the same transaction.
    ///
    /// [`CellDep`]: ../ckb_types/packed/struct.CellDep.html
    #[error("DuplicateCellDeps({out_point})")]
    DuplicateCellDeps {
        /// The out-point of that duplicated [`CellDep`].
        ///
        /// [`CellDep`]: ../ckb_types/packed/struct.CellDep.html
        out_point: OutPoint,
    },

    /// There are duplicated `HeaderDep` within the same transaction.
    #[error("DuplicateHeaderDeps({hash})")]
    DuplicateHeaderDeps {
        /// The block hash of that duplicated `HeaderDep.`
        hash: Byte32,
    },

    /// The length of outputs is not equal to the length of outputs-data (`outputs.len() != outputs_data.len()`).
    #[error("OutputsDataLengthMismatch: expected outputs data length ({outputs_data_len}) = outputs length ({outputs_len})")]
    OutputsDataLengthMismatch {
        /// The length of outputs.
        outputs_len: usize,
        /// The length of outputs-data.
        outputs_data_len: usize,
    },

    /// Error dues to the the fact that the since rule is not respected.
    ///
    /// See also [0017-tx-valid-since](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md).
    #[error("InvalidSince(Inputs[{index}]): the field since is invalid")]
    InvalidSince {
        /// The index of input with invalid since field
        index: usize,
    },

    /// The transaction is not mature yet, according to the `since` rule.
    ///
    /// See also [0017-tx-valid-since](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md).
    #[error(
        "Immature(Inputs[{index}]): the transaction is immature because of the since requirement"
    )]
    Immature {
        /// The index of input with immature `since` field.
        index: usize,
    },

    /// The transaction is not mature yet, according to the cellbase maturity rule.
    #[error("CellbaseImmaturity({inner}[{index}])")]
    CellbaseImmaturity {
        /// The transaction field that causes the error.
        /// It should be `TransactionErrorSource::Inputs` or `TransactionErrorSource::CellDeps`. It does not allow using an immature cell as input out-point and dependency out-point.
        inner: TransactionErrorSource,
        /// The index of immature input out-point or dependency out-point.
        index: usize,
    },

    /// The transaction version does not match with the system expected.
    #[error("MismatchedVersion: expected {}, got {}", expected, actual)]
    MismatchedVersion {
        /// The expected transaction version.
        expected: Version,
        /// The actual transaction version.
        actual: Version,
    },

    /// The transaction size exceeds limit.
    #[error("ExceededMaximumBlockBytes: expected transaction serialized size ({actual}) < block size limit ({limit})")]
    ExceededMaximumBlockBytes {
        /// The limited transaction size.
        limit: u64,
        /// The actual transaction size.
        actual: u64,
    },

    /// The compatible error.
    #[error("Compatible: the feature \"{feature}\" is used in current transaction but not enabled in current chain")]
    Compatible {
        /// The feature name.
        feature: &'static str,
    },

    /// The internal error.
    #[error("Internal: {description}, this error shouldn't happen, please report this bug to developers.")]
    Internal {
        /// The error description
        description: String,
    },
}

impl_error_conversion_with_kind!(TransactionError, ErrorKind::Transaction, Error);

impl TransactionError {
    /// Returns whether this transaction error indicates that the transaction is malformed.
    pub fn is_malformed_tx(&self) -> bool {
        match self {
            TransactionError::OutputsSumOverflow { .. }
            | TransactionError::DuplicateCellDeps { .. }
            | TransactionError::DuplicateHeaderDeps { .. }
            | TransactionError::Empty { .. }
            | TransactionError::InsufficientCellCapacity { .. }
            | TransactionError::InvalidSince { .. }
            | TransactionError::ExceededMaximumBlockBytes { .. }
            | TransactionError::OutputsDataLengthMismatch { .. } => true,

            TransactionError::Immature { .. }
            | TransactionError::CellbaseImmaturity { .. }
            | TransactionError::MismatchedVersion { .. }
            | TransactionError::Compatible { .. }
            | TransactionError::Internal { .. } => false,
        }
    }
}

impl OutPointError {
    /// Returns true if the error is an unknown out_point.
    pub fn is_unknown(&self) -> bool {
        matches!(self, OutPointError::Unknown(_))
    }
}
