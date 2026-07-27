use super::{NotifyTxBatch, NotifyTxBatchError};
use ckb_types::core::TransactionBuilder;

#[test]
fn notify_tx_batch_rejects_count_before_dispatch() {
    let tx = TransactionBuilder::default().build();
    let error = NotifyTxBatch::try_new_with_limits(vec![tx.clone(), tx], 1, usize::MAX)
        .expect_err("two transactions exceed the fixture count bound");
    assert_eq!(
        error,
        NotifyTxBatchError::TooMany {
            actual: 2,
            maximum: 1,
        }
    );
}

#[test]
fn notify_tx_batch_rejects_bytes_before_dispatch() {
    let tx = TransactionBuilder::default().build();
    let error = NotifyTxBatch::try_new_with_limits(vec![tx], 1, 0)
        .expect_err("a non-empty transaction exceeds the fixture byte bound");
    assert!(matches!(
        error,
        NotifyTxBatchError::TooLarge {
            actual: 1..,
            maximum: 0,
        }
    ));
}
