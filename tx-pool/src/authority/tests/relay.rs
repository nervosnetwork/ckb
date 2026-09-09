use super::super::relay::{
    RelayMailboxConfigError, RelayMailboxDisposition, authority_relay_mailbox,
    production_authority_relay_mailbox,
};
use crate::service::TxVerificationResult;
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use std::{collections::HashSet, mem::size_of, time::Duration};
const TEST_BYTES: usize = 16 * 1024;
const TEST_MAX_PARENTS: usize = 64;

#[tokio::test]
async fn uak_relay_mailbox_coalesces_ordinary_wake_at_the_high_watermark() {
    let (sink, receiver) = authority_relay_mailbox(4, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
    assert_eq!(
        sink.publish(TxVerificationResult::Reject {
            tx_hash: Byte32::new([1; 32]),
        }),
        RelayMailboxDisposition::Exact
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), receiver.wait_for_drain())
            .await
            .is_err(),
        "sparse ordinary results remain on the periodic batching path"
    );

    assert_eq!(
        sink.publish(TxVerificationResult::Reject {
            tx_hash: Byte32::new([2; 32]),
        }),
        RelayMailboxDisposition::Exact
    );
    tokio::time::timeout(Duration::from_secs(1), receiver.wait_for_drain())
        .await
        .expect("crossing the high watermark wakes the sole consumer");

    assert_eq!(
        sink.publish(TxVerificationResult::Reject {
            tx_hash: Byte32::new([3; 32]),
        }),
        RelayMailboxDisposition::Exact
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), receiver.wait_for_drain())
            .await
            .is_err(),
        "one occupied high-water interval produces at most one wake"
    );
}

#[tokio::test]
async fn uak_relay_mailbox_wakes_promptly_for_order_barriers() {
    let (sink, receiver) = authority_relay_mailbox(4, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
    assert_eq!(
        sink.publish(TxVerificationResult::GenerationReset),
        RelayMailboxDisposition::Exact
    );
    tokio::time::timeout(Duration::from_secs(1), receiver.wait_for_drain())
        .await
        .expect("a generation reset wakes the sole consumer immediately");
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::GenerationReset)
    ));

    assert_eq!(
        sink.publish(TxVerificationResult::UnknownParents {
            peer: PeerIndex::from(12),
            parents: [Byte32::new([7; 32])].into_iter().collect(),
        }),
        RelayMailboxDisposition::Exact
    );
    tokio::time::timeout(Duration::from_secs(1), receiver.wait_for_drain())
        .await
        .expect("a missing-parent request wakes the sole consumer immediately");
}

#[test]
fn uak_production_relay_mailbox_fits_reset_and_one_maximum_parent_frontier() {
    let (sink, receiver) = production_authority_relay_mailbox(2, TEST_MAX_PARENTS)
        .expect("the production formula reserves one indivisible frontier behind reset");
    for byte in [1, 2] {
        assert_eq!(
            sink.publish(TxVerificationResult::Reject {
                tx_hash: Byte32::new([byte; 32]),
            }),
            RelayMailboxDisposition::Exact
        );
    }
    let parents = (0..TEST_MAX_PARENTS)
        .map(|index| {
            let mut hash = [0u8; 32];
            hash[..size_of::<usize>()].copy_from_slice(&index.to_le_bytes());
            Byte32::new(hash)
        })
        .collect::<HashSet<_>>();
    assert_eq!(
        sink.publish(TxVerificationResult::UnknownParents {
            peer: PeerIndex::from(10),
            parents,
        }),
        RelayMailboxDisposition::Reconciled
    );
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::GenerationReset)
    ));
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::UnknownParents { parents, .. })
            if parents.len() == TEST_MAX_PARENTS
    ));
}

#[test]
fn uak_relay_mailbox_bounds_oversized_parent_detail_without_blocking() {
    assert!(matches!(
        authority_relay_mailbox(2, 256, 32),
        Err(RelayMailboxConfigError::ByteLimit)
    ));
    let (sink, receiver) =
        authority_relay_mailbox(2, 256, 0).expect("the defensive fixture declares no parents");
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
    let (sink, receiver) = authority_relay_mailbox(2, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
    drop(receiver);
    assert_eq!(
        sink.publish(TxVerificationResult::GenerationReset),
        RelayMailboxDisposition::Disconnected
    );
}

#[test]
fn uak_relay_mailbox_accounting_mismatch_rebuilds_instead_of_saturating() {
    for corrupted_bytes in [0, usize::MAX] {
        let (sink, receiver) = authority_relay_mailbox(2, TEST_BYTES, TEST_MAX_PARENTS)
            .expect("the bounded relay mailbox fixture is valid");
        assert_eq!(
            sink.publish(TxVerificationResult::Reject {
                tx_hash: Byte32::new([7; 32]),
            }),
            RelayMailboxDisposition::Exact
        );
        receiver.corrupt_bytes_for_test(corrupted_bytes);

        assert!(matches!(
            receiver.try_recv(),
            Some(TxVerificationResult::GenerationReset)
        ));
        assert_eq!(receiver.observation(), (0, 0));
    }
}
