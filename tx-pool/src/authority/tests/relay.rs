use super::super::relay::{RelayMailboxDisposition, authority_relay_mailbox};
use crate::service::TxVerificationResult;
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use std::collections::HashSet;

const TEST_BYTES: usize = 16 * 1024;

#[test]
fn uak_relay_mailbox_preserves_exact_order_within_its_bound() {
    let (sink, receiver) =
        authority_relay_mailbox(4, TEST_BYTES).expect("the bounded relay mailbox fixture is valid");
    let first = Byte32::new([1; 32]);
    let second = Byte32::new([2; 32]);
    assert_eq!(
        sink.publish(TxVerificationResult::Reject {
            tx_hash: first.clone(),
        }),
        RelayMailboxDisposition::Exact
    );
    assert_eq!(
        sink.publish(TxVerificationResult::Ok {
            original_peer: None,
            tx_hash: second.clone(),
        }),
        RelayMailboxDisposition::Exact
    );
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == first
    ));
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::Ok { tx_hash, .. }) if tx_hash == second
    ));
    assert!(receiver.try_recv().is_none());
}

#[test]
fn uak_relay_mailbox_overflow_orders_reset_before_the_current_result() {
    let (sink, receiver) =
        authority_relay_mailbox(2, TEST_BYTES).expect("the bounded relay mailbox fixture is valid");
    for byte in [1, 2] {
        assert_eq!(
            sink.publish(TxVerificationResult::Reject {
                tx_hash: Byte32::new([byte; 32]),
            }),
            RelayMailboxDisposition::Exact
        );
    }
    let current = Byte32::new([3; 32]);
    assert_eq!(
        sink.publish(TxVerificationResult::Reject {
            tx_hash: current.clone(),
        }),
        RelayMailboxDisposition::Reconciled
    );
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::GenerationReset)
    ));
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == current
    ));
    assert_eq!(receiver.observation(), (0, 0));
}

#[test]
fn uak_relay_mailbox_bounds_oversized_parent_detail_without_blocking() {
    let (sink, receiver) =
        authority_relay_mailbox(2, 256).expect("the narrow mailbox can retain two fixed envelopes");
    let parents = (0u8..32)
        .map(|byte| Byte32::new([byte; 32]))
        .collect::<HashSet<_>>();
    assert_eq!(
        sink.publish(TxVerificationResult::UnknownParents {
            peer: PeerIndex::from(9),
            parents,
        }),
        RelayMailboxDisposition::Unavailable
    );
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::GenerationReset)
    ));
    assert!(receiver.try_recv().is_none());
}

#[test]
fn uak_relay_mailbox_disconnect_is_a_stable_local_disposition() {
    let (sink, receiver) =
        authority_relay_mailbox(2, TEST_BYTES).expect("the bounded relay mailbox fixture is valid");
    drop(receiver);
    assert_eq!(
        sink.publish(TxVerificationResult::GenerationReset),
        RelayMailboxDisposition::Disconnected
    );
}
