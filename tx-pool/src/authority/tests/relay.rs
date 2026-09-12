use super::super::relay::{
    RelayMailboxConfigError, RelayMailboxDisposition, authority_relay_mailbox,
    production_authority_relay_mailbox,
};
use crate::service::TxVerificationResult;
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use futures_util::FutureExt;
use std::{collections::HashSet, mem::size_of};
const TEST_BYTES: usize = 16 * 1024;
const TEST_MAX_PARENTS: usize = 64;

#[test]
fn uak_relay_mailbox_coalesces_and_rearms_ordinary_high_watermark_wakes() {
    let (sink, receiver) = authority_relay_mailbox(4, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
    for batch in [0, 3] {
        for (index, wakes) in [(1, false), (2, true), (3, false)] {
            assert_eq!(
                sink.publish(TxVerificationResult::Reject {
                    tx_hash: Byte32::new([batch + index; 32]),
                }),
                RelayMailboxDisposition::Exact
            );
            assert_eq!(
                receiver.wait_for_drain().now_or_never().is_some(),
                wakes,
                "only crossing the high watermark signals an ordinary batch"
            );
        }
        // Draining below the watermark must rearm the next occupied interval.
        assert_eq!(receiver.drain(3).len(), 3);
    }
}

#[test]
fn uak_relay_mailbox_retains_early_wakes_for_order_barriers() {
    let (sink, receiver) = authority_relay_mailbox(4, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
    assert_eq!(
        sink.publish(TxVerificationResult::GenerationReset),
        RelayMailboxDisposition::Exact
    );
    assert!(receiver.wait_for_drain().now_or_never().is_some());
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
    assert!(receiver.wait_for_drain().now_or_never().is_some());
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

#[test]
fn relay_batch_drain_matches_single_receive_order_for_every_prefix() {
    for limit in [0, 1, 2, 3, 4, usize::MAX] {
        let (sink, receiver) = authority_relay_mailbox(4, TEST_BYTES, TEST_MAX_PARENTS).unwrap();
        let parents: HashSet<_> = [Byte32::new([9; 32])].into_iter().collect();
        for result in [
            TxVerificationResult::Reject {
                tx_hash: Byte32::new([1; 32]),
            },
            TxVerificationResult::GenerationReset,
            TxVerificationResult::UnknownParents {
                peer: 3.into(),
                parents: parents.clone(),
            },
            TxVerificationResult::Ok {
                original_peer: Some(4.into()),
                tx_hash: Byte32::new([4; 32]),
            },
        ] {
            assert_eq!(sink.publish(result), RelayMailboxDisposition::Exact);
        }
        let mut drained = receiver.drain(limit);
        assert_eq!(drained.len(), limit.min(4));
        assert_eq!(receiver.observation().0, 4 - drained.len());
        drained.extend(std::iter::from_fn(|| receiver.try_recv()));
        let [first, second, third, fourth]: [TxVerificationResult; 4] = drained.try_into().unwrap();
        assert!(
            matches!(first, TxVerificationResult::Reject { tx_hash } if tx_hash == Byte32::new([1; 32]))
        );
        assert!(matches!(second, TxVerificationResult::GenerationReset));
        assert!(
            matches!(third, TxVerificationResult::UnknownParents { peer, parents: found } if peer == PeerIndex::from(3) && found == parents)
        );
        assert!(
            matches!(fourth, TxVerificationResult::Ok { original_peer, tx_hash } if original_peer == Some(PeerIndex::from(4)) && tx_hash == Byte32::new([4; 32]))
        );
        assert_eq!(receiver.observation(), (0, 0));
    }
}

#[test]
fn relay_batch_drain_preserves_overflow_and_accounting_resets() {
    let (sink, receiver) = authority_relay_mailbox(2, TEST_BYTES, TEST_MAX_PARENTS).unwrap();
    for byte in [1, 2, 3] {
        sink.publish(TxVerificationResult::Reject {
            tx_hash: Byte32::new([byte; 32]),
        });
    }
    assert!(matches!(
        receiver.drain(1).as_slice(),
        [TxVerificationResult::GenerationReset]
    ));
    assert!(
        matches!(receiver.drain(8).as_slice(), [TxVerificationResult::Reject { tx_hash }] if *tx_hash == Byte32::new([3; 32]))
    );
    assert_eq!(receiver.observation(), (0, 0));

    for corrupted in [0, usize::MAX] {
        let (sink, receiver) = authority_relay_mailbox(2, TEST_BYTES, TEST_MAX_PARENTS).unwrap();
        sink.publish(TxVerificationResult::Reject {
            tx_hash: Byte32::new([7; 32]),
        });
        receiver.corrupt_bytes_for_test(corrupted);
        assert!(matches!(
            receiver.drain(2).as_slice(),
            [TxVerificationResult::GenerationReset]
        ));
        assert_eq!(receiver.observation(), (0, 0));
    }
}
