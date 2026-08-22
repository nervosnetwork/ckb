use super::super::relay::{
    RelayMailboxConfigError, RelayMailboxDisposition, authority_relay_mailbox,
    production_authority_relay_mailbox,
};
use super::super::service::AuthorityRelayDrain;
use super::super::{
    plan::TxPoolAuthority,
    read::RelayParentRebuildError,
    runtime::AuthorityRuntime,
    state::{DependencyKey, RawTxHash, ValidatedAdmission, WorkPermit},
};
use super::foundation::{
    admit_remote_until, apply_plan, genesis_snapshot, limits, owner_version, runtime_config,
    take_resolve_work, tx,
};
use crate::service::TxVerificationResult;
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint};
use std::time::Duration;
use std::{collections::HashSet, mem::size_of, num::NonZeroUsize};

const TEST_BYTES: usize = 16 * 1024;
const TEST_MAX_PARENTS: usize = 64;

fn wait_for_parents(
    authority: &mut TxPoolAuthority,
    hash: &RawTxHash,
    parents: Vec<DependencyKey>,
) {
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                hash,
                owner_version(authority, hash),
                WorkPermit::ResolveOnly,
            )
            .expect("the remote fixture checks out for resolve")
            .apply(),
    );
    apply_plan(
        authority
            .apply_settlement(
                work.missing(parents)
                    .expect("the missing-parent fixture is nonempty and bounded"),
            )
            .expect("the missing-parent fixture enters one committed wait"),
    );
}

fn runtime_with(authority: TxPoolAuthority) -> AuthorityRuntime {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    runtime.with_authority_for_foundation(|slot| *slot = authority);
    runtime
}

#[test]
fn uak_relay_mailbox_preserves_exact_order_within_its_bound() {
    let (sink, receiver) = authority_relay_mailbox(4, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
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
fn uak_relay_mailbox_overflow_orders_reset_before_the_current_result() {
    let (sink, receiver) = authority_relay_mailbox(2, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
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
fn uak_relay_mailbox_keeps_a_max_frontier_after_reconciliation() {
    let (sink, receiver) = authority_relay_mailbox(2, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("assembly proves reset plus one maximum parent frontier fits");
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
            peer: PeerIndex::from(8),
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
fn uak_relay_parent_rebuild_pages_the_authoritative_missing_level() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let _queued = admit_remote_until(&mut authority, 901, 41, 1);
    let first_waiter = admit_remote_until(&mut authority, 902, 42, 2);
    let second_waiter = admit_remote_until(&mut authority, 903, 43, 3);
    let first_parent = Byte32::new([0x11; 32]);
    let second_parent = Byte32::new([0x22; 32]);
    wait_for_parents(
        &mut authority,
        &first_waiter,
        vec![
            DependencyKey::Cell(OutPoint::new(second_parent.clone(), 0)),
            DependencyKey::Cell(OutPoint::new(first_parent.clone(), 1)),
            DependencyKey::Cell(OutPoint::new(first_parent.clone(), 0)),
        ],
    );
    wait_for_parents(
        &mut authority,
        &second_waiter,
        vec![DependencyKey::Header(Byte32::new([0x33; 32]))],
    );
    let runtime = runtime_with(authority);
    let reader = runtime.relay_parent_reader();
    let scan_limit = NonZeroUsize::new(1).expect("the scan fixture is nonzero");
    let mut cursor = None;
    let mut requests = Vec::new();
    let mut pages = 0usize;

    let completed_cut = loop {
        let page = reader
            .page(cursor, scan_limit)
            .expect("one unchanged authority cut pages deterministically");
        let (cut, page_requests, next) = page.into_parts();
        pages += 1;
        requests.extend(page_requests);
        if let Some(next) = next {
            cursor = Some(next);
        } else {
            break cut;
        }
    };

    assert_eq!(pages, 3, "queued owners still consume the bounded scan");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].peer(), PeerIndex::from(42));
    assert_eq!(
        requests[0].parents(),
        &[RawTxHash(first_parent), RawTxHash(second_parent),]
    );
    assert!(reader.cut_is_current(&completed_cut));
}

#[test]
fn uak_relay_parent_rebuild_ignores_unrelated_changes_but_restarts_after_level_change() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = admit_remote_until(&mut authority, 904, 44, 1);
    let second = admit_remote_until(&mut authority, 905, 45, 2);
    wait_for_parents(
        &mut authority,
        &first,
        vec![DependencyKey::Cell(OutPoint::new(
            Byte32::new([0x44; 32]),
            0,
        ))],
    );
    wait_for_parents(
        &mut authority,
        &second,
        vec![DependencyKey::Cell(OutPoint::new(
            Byte32::new([0x45; 32]),
            0,
        ))],
    );
    let runtime = runtime_with(authority);
    let reader = runtime.relay_parent_reader();
    let scan_limit = NonZeroUsize::new(1).expect("the scan fixture is nonzero");
    let first_page = reader
        .page(None, scan_limit)
        .expect("the first page captures one coherent cut");
    let (cut, _requests, cursor) = first_page.into_parts();
    let cursor = cursor.expect("a second Remote owner requires a continuation");

    runtime
        .queue_generation_reset_for_foundation()
        .expect("effect-only publication is independent of relay parent state");
    assert!(reader.cut_is_current(&cut));

    runtime.with_authority_for_foundation(|authority| {
        apply_plan(
            authority
                .plan_admission(
                    ValidatedAdmission::proposal(tx(907))
                        .expect("the trusted proposal fixture is valid"),
                )
                .expect("an unrelated trusted owner commits"),
        );
    });
    assert!(reader.cut_is_current(&cut));

    let new_owner = runtime
        .with_authority_for_foundation(|authority| admit_remote_until(authority, 906, 46, 3));
    assert!(reader.cut_is_current(&cut));

    runtime.with_authority_for_foundation(|authority| {
        wait_for_parents(
            authority,
            &new_owner,
            vec![DependencyKey::Cell(OutPoint::new(
                Byte32::new([0x46; 32]),
                0,
            ))],
        );
    });

    assert!(!reader.cut_is_current(&cut));
    assert_eq!(
        reader.page(Some(cursor), scan_limit).err(),
        Some(RelayParentRebuildError::StaleCut)
    );
}

#[test]
fn uak_relay_reset_rebuilds_every_still_live_missing_parent_request() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let waiter = admit_remote_until(&mut authority, 908, 47, 10);
    let missing_parent = Byte32::new([0x47; 32]);
    wait_for_parents(
        &mut authority,
        &waiter,
        vec![DependencyKey::Cell(OutPoint::new(
            missing_parent.clone(),
            0,
        ))],
    );
    let runtime = runtime_with(authority);
    let (sink, receiver) = authority_relay_mailbox(2, TEST_BYTES, TEST_MAX_PARENTS)
        .expect("the bounded relay mailbox fixture is valid");
    let scan_limit = NonZeroUsize::new(1).expect("the scan fixture is nonzero");
    let drain = AuthorityRelayDrain::new(receiver, runtime.relay_parent_reader(), scan_limit);

    let mut lost_parents = HashSet::new();
    lost_parents.insert(missing_parent.clone());
    assert_eq!(
        sink.publish(TxVerificationResult::UnknownParents {
            peer: PeerIndex::from(47),
            parents: lost_parents,
        }),
        RelayMailboxDisposition::Exact
    );
    assert_eq!(
        sink.publish(TxVerificationResult::Reject {
            tx_hash: Byte32::new([0x81; 32]),
        }),
        RelayMailboxDisposition::Exact
    );
    let retained = Byte32::new([0x82; 32]);
    assert_eq!(
        sink.publish(TxVerificationResult::Reject {
            tx_hash: retained.clone(),
        }),
        RelayMailboxDisposition::Reconciled
    );

    assert!(matches!(
        drain.try_recv(),
        Some(TxVerificationResult::GenerationReset)
    ));
    assert!(matches!(
        drain.try_recv(),
        Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == retained
    ));
    assert!(matches!(
        drain.try_recv(),
        Some(TxVerificationResult::UnknownParents { peer, parents })
            if peer == PeerIndex::from(47) && parents == HashSet::from([missing_parent])
    ));
    assert!(drain.try_recv().is_none());
}
