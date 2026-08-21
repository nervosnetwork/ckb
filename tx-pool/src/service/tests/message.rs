use super::{
    BoundedIdentifierSequenceError, BoundedProposalIds, BoundedTransaction,
    BoundedTransactionError, BoundedTransactionHashes, ExternalOperationClass, Message,
    NotifyTxBatch, NotifyTxBatchError,
};
use crate::service::Request;
use ckb_channel::oneshot;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, TransactionBuilder, TransactionView, tx_pool::TRANSACTION_SIZE_LIMIT},
    packed::{Byte32, ProposalShortId},
    prelude::{Entity, Pack},
};
use std::sync::Arc;

#[test]
fn external_operation_class_is_bound_to_the_real_remove_message_variant() {
    let (responder, _response) = oneshot::channel();
    let message = Message::RemoveLocalTx(Request::call(Byte32::default(), responder));
    assert_eq!(
        message.external_operation_class(),
        ExternalOperationClass::RemoveLocalTx
    );
}

impl NotifyTxBatch {
    pub(crate) fn into_transactions_for_test(self) -> Vec<TransactionView> {
        self.transactions
            .into_iter()
            .map(|transaction| Arc::unwrap_or_clone(transaction.into_transaction()))
            .collect()
    }
}

#[test]
fn bounded_transaction_preserves_the_exact_valid_transaction() {
    let transaction = TransactionBuilder::default()
        .output_data(Bytes::from_static(b"bounded").pack())
        .build();
    let expected_bytes = transaction.data().total_size();
    let bounded = BoundedTransaction::try_new(transaction.clone())
        .expect("a small transaction fits the protocol bound");

    assert_eq!(bounded.payload_bytes(), expected_bytes);
    assert_eq!(bounded.into_transaction().as_ref(), &transaction);
}

#[test]
fn bounded_transaction_owns_one_compact_backing_allocation() {
    let outer = BlockBuilder::default()
        .transaction(
            TransactionBuilder::default()
                .witness(Bytes::from(vec![7; 64 * 1024]))
                .build(),
        )
        .build();
    let outer_data = outer.data();
    let outer_start = outer_data.as_slice().as_ptr() as usize;
    let outer_end = outer_start + outer_data.as_slice().len();
    let shared = outer
        .transactions()
        .into_iter()
        .next()
        .expect("fixture block has one transaction");
    let shared_start = shared.data().as_slice().as_ptr() as usize;
    assert!((outer_start..outer_end).contains(&shared_start));

    let transaction = BoundedTransaction::try_new(shared)
        .expect("the source slice fits the protocol residency bound")
        .into_transaction();
    let data = transaction.data();
    let hash = transaction.hash();
    let witness_hash = transaction.witness_hash();
    let data_start = data.as_slice().as_ptr() as usize;
    let hash_start = hash.as_slice().as_ptr() as usize;
    let witness_hash_start = witness_hash.as_slice().as_ptr() as usize;
    assert!(data_start < outer_start || data_start >= outer_end);
    assert_eq!(hash_start, data_start + data.as_slice().len());
    assert_eq!(
        witness_hash_start,
        hash_start + hash.as_slice().len(),
        "transaction and both cached hashes must share one exact carrier"
    );
}

#[test]
fn bounded_transaction_rejects_the_protocol_size_boundary_before_enqueue() {
    let transaction = TransactionBuilder::default()
        .output_data(Bytes::from(vec![0; TRANSACTION_SIZE_LIMIT as usize + 1]).pack())
        .build();
    let actual = transaction.data().serialized_size_in_block() as u64;
    let error = BoundedTransaction::try_new(transaction)
        .expect_err("an oversized transaction cannot cross the service channel");

    assert!(matches!(
        error,
        BoundedTransactionError::TooLarge {
            actual: observed,
            maximum: TRANSACTION_SIZE_LIMIT,
        } if observed == actual
    ));
}

#[test]
fn bounded_proposal_ids_reject_count_before_normalization() {
    let error = BoundedProposalIds::try_from_vec_with_limit(
        vec![ProposalShortId::default(), ProposalShortId::default()],
        1,
    )
    .expect_err("two IDs exceed the fixture count bound");

    assert_eq!(
        error,
        BoundedIdentifierSequenceError::TooMany {
            actual: 2,
            maximum: 1,
        }
    );
}

#[test]
fn bounded_transaction_hashes_reject_count_before_residency() {
    let error = BoundedTransactionHashes::try_from_iter_with_limit(
        [Byte32::new([1; 32]), Byte32::new([2; 32])].into_iter(),
        1,
    )
    .expect_err("two full hashes exceed the fixture count bound");
    assert_eq!(
        error,
        BoundedIdentifierSequenceError::TooMany {
            actual: 2,
            maximum: 1,
        }
    );
}

#[test]
fn bounded_transaction_hashes_preserve_full_identity_in_canonical_order() {
    let low = Byte32::new([1; 32]);
    let high = Byte32::new([2; 32]);
    let bounded = BoundedTransactionHashes::try_from_iter_with_limit(
        [high.clone(), low.clone()].into_iter(),
        2,
    )
    .expect("the finite full-hash sequence is valid");
    let compact = bounded.into_vec();
    assert_eq!(compact, vec![low, high]);
    let mut starts = compact
        .iter()
        .map(|hash| hash.as_slice().as_ptr() as usize)
        .collect::<Vec<_>>();
    starts.sort_unstable();
    assert_eq!(
        starts[1],
        starts[0] + compact[0].as_slice().len(),
        "full hashes must share one exact bounded backing allocation"
    );
}

#[test]
fn bounded_proposal_ids_preserve_the_exact_finite_sequence() {
    let ids = vec![ProposalShortId::new([1; 10]), ProposalShortId::new([2; 10])];
    let bounded = BoundedProposalIds::try_from_vec_with_limit(ids.clone(), ids.len())
        .expect("the finite proposal sequence is valid");
    let compact = bounded.into_vec();
    assert_eq!(compact, ids);
    assert_eq!(
        compact[1].as_slice().as_ptr() as usize,
        compact[0].as_slice().as_ptr() as usize + compact[0].as_slice().len(),
        "proposal IDs must share one order-preserving byte carrier"
    );
}

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

#[test]
fn notify_tx_batch_rejects_an_individually_oversized_transaction() {
    let transaction = TransactionBuilder::default()
        .output_data(Bytes::from(vec![0; TRANSACTION_SIZE_LIMIT as usize + 1]).pack())
        .build();
    let actual = transaction.data().serialized_size_in_block() as u64;
    let error = NotifyTxBatch::try_new_with_limits(vec![transaction], 1, usize::MAX)
        .expect_err("the batch byte sum cannot hide an invalid transaction");
    assert!(matches!(
        error,
        NotifyTxBatchError::TransactionTooLarge {
            actual: observed,
            maximum: TRANSACTION_SIZE_LIMIT,
        } if observed == actual
    ));
}
