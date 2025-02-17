use ckb_error::{ErrorKind, InternalErrorKind, OtherError, SilentError as DefaultError};

use crate::core::{
    error::{OutPointError, TransactionError, TransactionErrorSource, ARGV_TOO_LONG_TEXT},
    tx_pool::Reject,
};

#[test]
fn test_if_is_malformed_tx() {
    let reject = Reject::LowFeeRate(Default::default(), 0, 0);
    assert!(!reject.is_malformed_tx());

    let reject = Reject::ExceededMaximumAncestorsCount;
    assert!(!reject.is_malformed_tx());

    let reject = Reject::ExceededTransactionSizeLimit(0, 0);
    assert!(!reject.is_malformed_tx());

    let reject = Reject::Full(Default::default());
    assert!(!reject.is_malformed_tx());

    let reject = Reject::Duplicated(Default::default());
    assert!(!reject.is_malformed_tx());

    let reject = Reject::Malformed(Default::default(), Default::default());
    assert!(reject.is_malformed_tx());

    for error in [
        OutPointError::Dead(Default::default()),
        OutPointError::Unknown(Default::default()),
        OutPointError::OutOfOrder(Default::default()),
        OutPointError::InvalidDepGroup(Default::default()),
        OutPointError::InvalidHeader(Default::default()),
    ] {
        let reject = Reject::Resolve(error);
        assert!(!reject.is_malformed_tx());
    }

    let error = OutPointError::OverMaxDepExpansionLimit;
    let reject = Reject::Resolve(error);
    assert!(reject.is_malformed_tx());

    for tx_error in vec![
        TransactionError::InsufficientCellCapacity {
            inner: TransactionErrorSource::Inputs,
            index: 0,
            occupied_capacity: Default::default(),
            capacity: Default::default(),
        },
        TransactionError::OutputsSumOverflow {
            inputs_sum: Default::default(),
            outputs_sum: Default::default(),
        },
        TransactionError::Empty {
            inner: TransactionErrorSource::Outputs,
        },
        TransactionError::DuplicateCellDeps {
            out_point: Default::default(),
        },
        TransactionError::DuplicateHeaderDeps {
            hash: Default::default(),
        },
        TransactionError::OutputsDataLengthMismatch {
            outputs_len: 0,
            outputs_data_len: 0,
        },
        TransactionError::InvalidSince { index: 0 },
        TransactionError::Immature { index: 0 },
        TransactionError::CellbaseImmaturity {
            inner: TransactionErrorSource::Witnesses,
            index: 0,
        },
        TransactionError::MismatchedVersion {
            expected: 0,
            actual: 0,
        },
        TransactionError::ExceededMaximumBlockBytes {
            limit: 0,
            actual: 0,
        },
        TransactionError::Compatible {
            feature: "feature-name",
        },
        TransactionError::Internal {
            description: "the-description".to_owned(),
        },
    ] {
        let is_malformed = tx_error.is_malformed_tx();
        let error_kind = ErrorKind::Transaction;
        let error = error_kind.because(tx_error);
        let reject = Reject::Verification(error);
        assert_eq!(reject.is_malformed_tx(), is_malformed);
    }

    {
        let error_kind = ErrorKind::Script;
        let error = error_kind.because(DefaultError);
        let reject = Reject::Verification(error);
        assert!(reject.is_malformed_tx());
    }

    {
        let error_kind = ErrorKind::Script;
        let error = error_kind.because(OtherError::new(ARGV_TOO_LONG_TEXT.to_string()));
        let reject = Reject::Verification(error);
        assert!(!reject.is_malformed_tx());
    }

    for error_kind in &[
        InternalErrorKind::CapacityOverflow,
        InternalErrorKind::DataCorrupted,
        InternalErrorKind::Database,
        InternalErrorKind::BlockAssembler,
        InternalErrorKind::VM,
        InternalErrorKind::System,
        InternalErrorKind::Config,
        InternalErrorKind::Other,
    ] {
        let is_malformed = *error_kind == InternalErrorKind::CapacityOverflow;
        let error = error_kind.because(DefaultError);
        let reject = Reject::Verification(error.into());
        assert_eq!(reject.is_malformed_tx(), is_malformed);
    }
}
