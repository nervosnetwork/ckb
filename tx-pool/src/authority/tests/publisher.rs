use super::super::{
    effect::{
        CommittedAcceptance, CommittedConflictOwner, CommittedEffect, CommittedEntrySnapshot,
        CommittedPeerCohortRevocation, CommittedRejection, EffectPolicy, ParentTransactionRequest,
        RejectionAudience,
    },
    plan::MembershipReject,
    publisher::{
        AuthorityEffectEndpoints, AuthorityEffectPublisherFaultKind, EndpointDisposition,
        RelayAction, RelayDisposition, compile_committed_effect, run_authority_effect_publisher,
    },
    rejection::CommittedPublicReject,
    runtime::AuthorityRuntime,
    state::{AcceptedStatus, RawTxHash},
};
use super::foundation::{genesis_snapshot, runtime_config, tx};
use crate::{
    callback::{CallbackEvent, Callbacks},
    component::entry::TxEntrySnapshot,
    error::Reject,
    network::DummyTxPoolNetwork,
    service::TxVerificationResult,
};
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Capacity, FeeRate},
    packed::{Byte32, OutPoint},
};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn entry(nonce: u64) -> CommittedEntrySnapshot {
    CommittedEntrySnapshot {
        tx: Arc::new(tx(nonce)),
        cycles: 11,
        size: 12,
        fee: Capacity::shannons(13),
        ancestors_size: 14,
        ancestors_fee: Capacity::shannons(15),
        ancestors_cycles: 16,
        ancestors_count: 17,
        descendants_fee: Capacity::shannons(18),
        descendants_size: 19,
        descendants_cycles: 20,
        descendants_count: 21,
        timestamp: 22,
    }
}

fn callback_snapshot(entry: &CommittedEntrySnapshot) -> TxEntrySnapshot {
    TxEntrySnapshot {
        transaction: entry.tx.as_ref().clone(),
        cycles: entry.cycles,
        size: entry.size,
        fee: entry.fee,
        ancestors_size: entry.ancestors_size,
        ancestors_fee: entry.ancestors_fee,
        ancestors_cycles: entry.ancestors_cycles,
        ancestors_count: entry.ancestors_count,
        descendants_fee: entry.descendants_fee,
        descendants_size: entry.descendants_size,
        descendants_cycles: entry.descendants_cycles,
        descendants_count: entry.descendants_count,
        timestamp: entry.timestamp,
    }
}

fn endpoints(
    relay: ckb_channel::Sender<TxVerificationResult>,
    callbacks: Arc<Callbacks>,
) -> AuthorityEffectEndpoints {
    AuthorityEffectEndpoints::new(Arc::new(DummyTxPoolNetwork), relay, callbacks, None)
        .expect("the publisher fixture can start its bounded callback worker")
}

#[test]
fn uak_effect_compiler_preserves_acceptance_and_chain_endpoint_semantics() {
    let ingress = PeerIndex::from(41);
    let accepted = entry(4_001);
    let expected_hash = accepted.tx.hash();
    let admission =
        compile_committed_effect(CommittedEffect::Accepted(CommittedAcceptance::Admission {
            entry: accepted.clone(),
            status: AcceptedStatus::Gap,
            ingress_peer: Some(ingress),
        }));
    let Some(CallbackEvent::Pending(snapshot)) = admission.callback else {
        panic!("Gap admission must retain the existing Pending callback contract");
    };
    assert_eq!(snapshot, callback_snapshot(&accepted));
    let Some(relay) = admission.relay else {
        panic!("admission must settle the relayer projection");
    };
    assert!(!relay.is_required());
    match relay.result {
        TxVerificationResult::Ok {
            original_peer,
            tx_hash,
        } => {
            assert_eq!(original_peer, Some(ingress));
            assert_eq!(tx_hash, expected_hash);
        }
        other => panic!("unexpected admission relay result: {other:?}"),
    }
    assert!(admission.recent_reject.is_none());
    assert!(admission.ban.is_none());

    let status_change = compile_committed_effect(CommittedEffect::Accepted(
        CommittedAcceptance::ChainStatusChange {
            entry: accepted.clone(),
            status: AcceptedStatus::Proposed,
        },
    ));
    let Some(CallbackEvent::Proposed(snapshot)) = status_change.callback else {
        panic!("chain proposal transition must publish Proposed");
    };
    assert_eq!(snapshot, callback_snapshot(&accepted));
    assert!(status_change.relay.is_none());

    let duplicate =
        compile_committed_effect(CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
            tx_hash: RawTxHash(expected_hash.clone()),
            requesting_peer: None,
        }));
    assert!(duplicate.callback.is_none());
    match duplicate.relay.map(|action| action.result) {
        Some(TxVerificationResult::Ok {
            original_peer,
            tx_hash,
        }) => {
            assert_eq!(original_peer, None);
            assert_eq!(tx_hash, expected_hash);
        }
        other => panic!("unexpected duplicate result: {other:?}"),
    }

    let committed = compile_committed_effect(CommittedEffect::ChainCommitted {
        tx_hash: RawTxHash(expected_hash.clone()),
        ingress_peer: ingress,
    });
    match committed.relay.map(|action| action.result) {
        Some(TxVerificationResult::Ok {
            original_peer,
            tx_hash,
        }) => {
            assert_eq!(original_peer, Some(ingress));
            assert_eq!(tx_hash, expected_hash);
        }
        other => panic!("unexpected chain-commit result: {other:?}"),
    }
    assert!(committed.callback.is_none());
}

#[test]
fn uak_effect_compiler_keeps_rejection_owner_and_peer_attribution_typed() {
    let ingress = PeerIndex::from(51);
    let blame = PeerIndex::from(52);
    let candidate = Arc::new(tx(4_101));
    let malformed = Reject::Malformed("script".to_owned(), "invalid encoding".to_owned());
    let validation =
        compile_committed_effect(CommittedEffect::Rejected(CommittedRejection::Validation {
            tx: Arc::clone(&candidate),
            audience: RejectionAudience::from_owner(Some(ingress), Some(blame)),
            reason: CommittedPublicReject::new(malformed),
        }));
    assert_eq!(
        validation
            .recent_reject
            .as_ref()
            .map(|action| &action.tx_hash),
        Some(&candidate.hash())
    );
    assert!(matches!(
        validation
            .recent_reject
            .as_ref()
            .map(|action| &action.reject),
        Some(Reject::Malformed(_, _))
    ));
    assert!(
        validation.ban.is_none(),
        "a generic rejection compiler cannot invent an uncommitted peer ban"
    );
    assert!(validation.callback.is_none());
    assert!(validation.relay.is_none());

    let membership =
        compile_committed_effect(CommittedEffect::Rejected(CommittedRejection::Membership {
            tx: Arc::clone(&candidate),
            audience: RejectionAudience::from_owner(Some(ingress), None),
            reason: MembershipReject::TooManyAncestors,
        }));
    assert!(membership.callback.is_none());
    assert!(membership.ban.is_none());
    assert!(matches!(
        membership
            .recent_reject
            .as_ref()
            .map(|action| &action.reject),
        Some(Reject::ExceededMaximumAncestorsCount)
    ));
    assert!(matches!(
        membership.relay.map(|action| action.result),
        Some(TxVerificationResult::Reject { .. })
    ));

    let duplicate =
        compile_committed_effect(CommittedEffect::Rejected(CommittedRejection::Validation {
            tx: Arc::clone(&candidate),
            audience: RejectionAudience::from_owner(Some(ingress), None),
            reason: CommittedPublicReject::new(Reject::Duplicated(candidate.hash())),
        }));
    assert!(
        duplicate.relay.is_none(),
        "a misclassified duplicate must never poison the relayer filter"
    );

    let victim = entry(4_102);
    let replacement =
        compile_committed_effect(CommittedEffect::Rejected(CommittedRejection::Replaced {
            entry: victim.clone(),
            audience: RejectionAudience::from_owner(Some(ingress), None),
            winner: RawTxHash(candidate.hash()),
        }));
    let Some(CallbackEvent::Reject(snapshot, Reject::RBFRejected(_))) = replacement.callback else {
        panic!("an accepted RBF victim must retain a rejection callback snapshot");
    };
    assert_eq!(snapshot, callback_snapshot(&victim));
    assert!(replacement.recent_reject.is_some());
    assert!(matches!(
        replacement.relay.map(|action| action.result),
        Some(TxVerificationResult::Reject { .. })
    ));

    let evicted = compile_committed_effect(CommittedEffect::Rejected(
        CommittedRejection::CapacityEvicted {
            entry: victim,
            audience: RejectionAudience::default(),
            fee_rate: FeeRate::from_u64(42),
        },
    ));
    assert!(matches!(
        evicted.callback,
        Some(CallbackEvent::Reject(_, Reject::Full(_)))
    ));
    assert!(evicted.recent_reject.is_none());
    assert!(matches!(
        evicted.relay.map(|action| action.result),
        Some(TxVerificationResult::Reject { .. })
    ));
}

#[test]
fn uak_effect_compiler_exhausts_conflict_cleanup_and_required_detail_variants() {
    let peer = PeerIndex::from(61);
    let candidate = Arc::new(tx(4_201));
    let accepted = entry(4_202);
    let out_point = OutPoint::default();

    let preaccepted = compile_committed_effect(CommittedEffect::Rejected(
        CommittedRejection::ChainConflict {
            owner: CommittedConflictOwner::PreAccepted(Arc::clone(&candidate)),
            audience: RejectionAudience::from_owner(Some(peer), None),
            out_point: out_point.clone(),
        },
    ));
    assert!(preaccepted.callback.is_none());
    assert!(matches!(
        preaccepted
            .recent_reject
            .as_ref()
            .map(|action| &action.reject),
        Some(Reject::Resolve(_))
    ));

    let accepted_conflict = compile_committed_effect(CommittedEffect::Rejected(
        CommittedRejection::ChainConflict {
            owner: CommittedConflictOwner::Accepted(accepted.clone()),
            audience: RejectionAudience::default(),
            out_point,
        },
    ));
    let Some(CallbackEvent::Reject(snapshot, Reject::Resolve(_))) = accepted_conflict.callback
    else {
        panic!("accepted chain conflict must publish its exact terminal snapshot");
    };
    assert_eq!(snapshot, callback_snapshot(&accepted));

    let culprit_reason = CommittedPublicReject::new(Reject::Malformed(
        "peer cohort fixture".to_owned(),
        String::new(),
    ));
    let revocation = CommittedPeerCohortRevocation::malformed_for_foundation(
        peer,
        RawTxHash(candidate.hash()),
        culprit_reason.clone(),
    )
    .expect("malformed evidence constructs peer-ban detail");
    let cleanup = compile_committed_effect(CommittedEffect::PeerCohortRevoked(revocation));
    assert!(cleanup.callback.is_none());
    assert!(cleanup.recent_reject.is_some());
    assert!(cleanup.ban.is_some_and(|ban| {
        ban.peer() == peer
            && ban
                .remaining_duration_at(std::time::Instant::now())
                .is_some()
    }));
    let relay = cleanup.relay.expect("cohort cleanup resets relay state");
    assert!(relay.is_required());
    assert!(matches!(
        relay.result,
        TxVerificationResult::GenerationReset
    ));

    let expiry = compile_committed_effect(CommittedEffect::RemoteExpired {
        tx_hash: RawTxHash(candidate.hash()),
        peer,
    });
    assert!(expiry.callback.is_none());
    assert!(expiry.recent_reject.is_none());
    assert!(expiry.ban.is_none());
    assert!(matches!(
        expiry.relay.map(|action| action.result),
        Some(TxVerificationResult::Reject { .. })
    ));

    let released_hash = RawTxHash(candidate.hash());
    let released = compile_committed_effect(CommittedEffect::RemoteIngressReleased {
        tx_hash: released_hash.clone(),
    });
    assert!(released.callback.is_none());
    assert!(released.recent_reject.is_none());
    assert!(released.ban.is_none());
    let released_relay = released
        .relay
        .expect("a released duplicate must leave the relayer known filter");
    assert!(!released_relay.is_required());
    assert!(matches!(
        released_relay.result,
        TxVerificationResult::Reject { tx_hash } if tx_hash == released_hash.0
    ));

    let first_parent = RawTxHash(Byte32::new([1; 32]));
    let second_parent = RawTxHash(Byte32::new([2; 32]));
    let request = ParentTransactionRequest::new(
        peer,
        Arc::from([first_parent.clone(), second_parent.clone()]),
    )
    .expect("a non-empty parent request is valid");
    let parents = compile_committed_effect(CommittedEffect::ParentTransactionsRequested(request));
    let Some(parent_relay) = parents.relay else {
        panic!("missing-parent detail must reach the relayer");
    };
    assert!(parent_relay.is_required());
    match parent_relay.result {
        TxVerificationResult::UnknownParents {
            peer: actual_peer,
            parents,
        } => {
            assert_eq!(actual_peer, peer);
            assert_eq!(
                parents,
                HashSet::from([first_parent.0.clone(), second_parent.0.clone()])
            );
        }
        other => panic!("unexpected missing-parent result: {other:?}"),
    }

    let reset = compile_committed_effect(CommittedEffect::GenerationReset);
    let Some(reset_relay) = reset.relay else {
        panic!("generation reset must reach the relayer");
    };
    assert!(reset_relay.is_required());
    assert!(matches!(
        reset_relay.result,
        TxVerificationResult::GenerationReset
    ));
}

#[tokio::test]
async fn uak_ordinary_relay_saturation_publishes_reset_before_disposal() {
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    relay_tx
        .send(TxVerificationResult::Reject {
            tx_hash: Byte32::zero(),
        })
        .expect("the relay fixture starts full");
    let endpoints = endpoints(relay_tx, Arc::new(Callbacks::new()));
    let publisher = tokio::spawn(async move {
        endpoints
            .publish_relay(RelayAction::ordinary(TxVerificationResult::Reject {
                tx_hash: Byte32::new([7; 32]),
            }))
            .await
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = relay_rx
        .try_recv()
        .expect("the original filler remains at the relay head");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), publisher)
            .await
            .expect("reset publication is bounded")
            .expect("the relay task remains healthy")
            .expect("the required sink remains connected"),
        RelayDisposition::Reconciled
    );
    assert!(matches!(
        relay_rx.try_recv(),
        Ok(TxVerificationResult::GenerationReset)
    ));
}

#[tokio::test]
async fn uak_required_parent_detail_never_degrades_under_relay_saturation() {
    let peer = PeerIndex::from(71);
    let expected = Byte32::new([8; 32]);
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    relay_tx
        .send(TxVerificationResult::GenerationReset)
        .expect("the relay fixture starts full");
    let endpoints = endpoints(relay_tx, Arc::new(Callbacks::new()));
    let expected_for_publisher = expected.clone();
    let publisher = tokio::spawn(async move {
        endpoints
            .publish_relay(RelayAction::required(
                TxVerificationResult::UnknownParents {
                    peer,
                    parents: HashSet::from([expected_for_publisher]),
                },
            ))
            .await
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = relay_rx
        .try_recv()
        .expect("the original filler remains at the relay head");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), publisher)
            .await
            .expect("required publication resumes when capacity returns")
            .expect("the relay task remains healthy")
            .expect("the required sink remains connected"),
        RelayDisposition::Exact
    );
    match relay_rx.try_recv() {
        Ok(TxVerificationResult::UnknownParents {
            peer: actual_peer,
            parents,
        }) => {
            assert_eq!(actual_peer, peer);
            assert_eq!(parents, HashSet::from([expected]));
        }
        other => panic!("required detail was replaced: {other:?}"),
    }
}

#[tokio::test]
async fn uak_relay_disconnect_is_typed_and_does_not_claim_publication() {
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    drop(relay_rx);
    let endpoints = endpoints(relay_tx, Arc::new(Callbacks::new()));
    let result = endpoints
        .publish_relay(RelayAction::ordinary(TxVerificationResult::GenerationReset))
        .await;
    assert!(matches!(
        result,
        Err(fault) if matches!(fault.kind, AuthorityEffectPublisherFaultKind::RelayDisconnected)
    ));
}

#[tokio::test]
async fn uak_publisher_relay_disconnect_retains_the_authority_head() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::RemoteExpired {
                tx_hash: RawTxHash(Byte32::new([12; 32])),
                peer: PeerIndex::from(84),
            },
        )
        .expect("the bounded fixture effect commits");
    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    drop(relay_rx);

    let result = run_authority_effect_publisher(
        runtime.clone(),
        endpoints(relay_tx, Arc::new(Callbacks::new())),
    )
    .await;
    assert!(matches!(
        result,
        Err(fault) if matches!(fault.kind, AuthorityEffectPublisherFaultKind::RelayDisconnected)
    ));
    let retained = runtime.effect_observation_for_foundation();
    assert_eq!(retained.active, None);
    assert_eq!(retained.queued.len(), 1);
    assert!(runtime.claim_effect_publisher().is_some());
}

#[test]
fn uak_effect_publisher_claim_is_move_only_and_exclusive() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    let first = runtime
        .claim_effect_publisher()
        .expect("the first publisher owns the sole claim");
    assert!(runtime.claim_effect_publisher().is_none());
    drop(first);
    assert!(runtime.claim_effect_publisher().is_some());
}

#[tokio::test]
async fn uak_cancelled_publisher_returns_the_complete_lease_to_the_fifo_head() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    let expected = RawTxHash(Byte32::new([9; 32]));
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::RemoteExpired {
                tx_hash: expected.clone(),
                peer: PeerIndex::from(81),
            },
        )
        .expect("the bounded fixture effect commits");

    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    relay_tx
        .send(TxVerificationResult::GenerationReset)
        .expect("the relay fixture starts full");
    let publisher = tokio::spawn(run_authority_effect_publisher(
        runtime.clone(),
        endpoints(relay_tx.clone(), Arc::new(Callbacks::new())),
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if runtime.effect_observation_for_foundation().active.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the publisher checks out the committed head");
    let active_sequence = runtime
        .effect_observation_for_foundation()
        .active
        .expect("the active lease has one sequence");

    publisher.abort();
    let abort = publisher.await;
    assert!(abort.is_err_and(|error| error.is_cancelled()));
    let retained = runtime.effect_observation_for_foundation();
    assert_eq!(retained.active, None);
    assert_eq!(retained.queued.first(), Some(&active_sequence));

    let _ = relay_rx
        .try_recv()
        .expect("the blocking filler is still present");
    runtime
        .close_effects()
        .expect("the producer side closes after cancellation retention");
    let replacement = tokio::spawn(run_authority_effect_publisher(
        runtime.clone(),
        endpoints(relay_tx, Arc::new(Callbacks::new())),
    ));
    match tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the replacement publisher drains the retained head")
    {
        TxVerificationResult::Reject { tx_hash } => assert_eq!(tx_hash, expected.0),
        other => panic!("unexpected retained publication: {other:?}"),
    }
    replacement
        .await
        .expect("the replacement publisher task remains healthy")
        .expect("the closed authority drains without a fault");
    assert!(runtime.effects_closed_and_drained());
}

#[tokio::test]
async fn uak_retained_batch_resumes_at_its_first_unprocessed_endpoint() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    let first = RawTxHash(Byte32::new([10; 32]));
    let second = RawTxHash(Byte32::new([11; 32]));
    runtime
        .queue_effects_for_foundation(
            EffectPolicy::Remote,
            vec![
                CommittedEffect::RemoteExpired {
                    tx_hash: first.clone(),
                    peer: PeerIndex::from(82),
                },
                CommittedEffect::RemoteExpired {
                    tx_hash: second.clone(),
                    peer: PeerIndex::from(83),
                },
            ],
        )
        .expect("the bounded two-effect batch commits atomically");

    let mut lease = runtime
        .wait_effect_checkout()
        .await
        .expect("effect checkout remains healthy")
        .expect("the committed batch is available");
    assert!(matches!(
        lease.current(),
        Some(work)
            if matches!(work.effect, CommittedEffect::RemoteExpired { tx_hash, .. } if tx_hash == &first)
    ));
    let first_effect_index = lease
        .current()
        .expect("the first effect has an endpoint cursor")
        .effect_index;
    loop {
        assert!(
            !lease
                .mark_current_processed()
                .expect("the first effect endpoint advances the typed local cursor")
        );
        if lease
            .current()
            .is_some_and(|work| work.effect_index != first_effect_index)
        {
            break;
        }
    }
    runtime
        .settle_effect(lease.retain())
        .expect("Retain commits the processed prefix into the sole authority");

    let mut retained = runtime
        .wait_effect_checkout()
        .await
        .expect("retained checkout remains healthy")
        .expect("the unfinished batch remains charged");
    assert!(matches!(
        retained.current(),
        Some(work)
            if matches!(work.effect, CommittedEffect::RemoteExpired { tx_hash, .. } if tx_hash == &second)
    ));
    loop {
        if retained
            .mark_current_processed()
            .expect("the suffix endpoint advances the typed local cursor")
        {
            break;
        }
    }
    runtime
        .settle_effect(
            retained
                .into_complete()
                .expect("the processed suffix creates a completed capability")
                .published(),
        )
        .expect("publishing the suffix releases the whole batch charge");
}

#[tokio::test]
async fn uak_cancelled_later_endpoint_does_not_replay_completed_callback() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    let victim = entry(4_303);
    let expected_hash = victim.tx.hash();
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::Rejected(CommittedRejection::CapacityEvicted {
                entry: victim,
                audience: RejectionAudience::default(),
                fee_rate: FeeRate::from_u64(42),
            }),
        )
        .expect("the bounded rejection effect commits");

    let callback_calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&callback_calls);
    let mut callbacks = Callbacks::new();
    callbacks.register_reject(Box::new(move |_, _| {
        observed_calls.fetch_add(1, Ordering::AcqRel);
    }));
    let callbacks = Arc::new(callbacks);

    let (relay_tx, relay_rx) = ckb_channel::bounded(1);
    relay_tx
        .send(TxVerificationResult::GenerationReset)
        .expect("the relay fixture starts full");
    let publisher = tokio::spawn(run_authority_effect_publisher(
        runtime.clone(),
        endpoints(relay_tx.clone(), Arc::clone(&callbacks)),
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while callback_calls.load(Ordering::Acquire) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the callback completes before the relay endpoint blocks");
    // The callback counter changes inside the foreign worker, before its
    // acknowledgement reaches the publisher. Give the publisher one bounded
    // scheduling turn to record that acknowledgement and enter the already
    // full relay endpoint; cancellation inside the callback's own action/ack
    // window is intentionally only at-least-once.
    tokio::time::sleep(Duration::from_millis(50)).await;

    publisher.abort();
    let abort = publisher.await;
    assert!(abort.is_err_and(|error| error.is_cancelled()));
    assert_eq!(callback_calls.load(Ordering::Acquire), 1);

    let _ = relay_rx
        .try_recv()
        .expect("the blocking filler remains ahead of the retained relay step");
    runtime
        .close_effects()
        .expect("the producer side closes after cancellation retention");
    let replacement = tokio::spawn(run_authority_effect_publisher(
        runtime.clone(),
        endpoints(relay_tx, callbacks),
    ));
    match tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the replacement publisher resumes at the retained relay endpoint")
    {
        TxVerificationResult::Reject { tx_hash } => assert_eq!(tx_hash, expected_hash),
        other => panic!("unexpected retained publication: {other:?}"),
    }
    replacement
        .await
        .expect("the replacement publisher task remains healthy")
        .expect("the closed authority drains without a fault");
    assert_eq!(
        callback_calls.load(Ordering::Acquire),
        1,
        "a completed endpoint must not replay after a later endpoint is cancelled"
    );
}

#[tokio::test]
async fn uak_unregistered_callback_is_not_dispatched_to_the_foreign_worker() {
    let (relay_tx, _relay_rx) = ckb_channel::bounded(1);
    let endpoints = endpoints(relay_tx, Arc::new(Callbacks::new()));
    let outcome =
        compile_committed_effect(CommittedEffect::Accepted(CommittedAcceptance::Admission {
            entry: entry(4_301),
            status: AcceptedStatus::Pending,
            ingress_peer: None,
        }));
    let mut reconciled = false;
    assert!(matches!(
        endpoints.publish(outcome, &mut reconciled).await,
        Ok(EndpointDisposition::Published)
    ));
    assert!(!reconciled);
}

#[tokio::test]
async fn uak_callback_uses_the_production_timeout_and_opens_one_stable_circuit() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut callbacks = Callbacks::new();
    let observed = Arc::clone(&calls);
    callbacks.register_pending(Box::new(move |_| {
        observed.fetch_add(1, Ordering::AcqRel);
        std::thread::sleep(Duration::from_millis(1_100));
    }));
    let (relay_tx, _relay_rx) = ckb_channel::bounded(1);
    let endpoints = endpoints(relay_tx, Arc::new(callbacks));
    let effect = CommittedEffect::Accepted(CommittedAcceptance::ChainStatusChange {
        entry: entry(4_302),
        status: AcceptedStatus::Pending,
    });

    let mut reconciled = false;
    let first = tokio::time::timeout(
        Duration::from_millis(1_500),
        endpoints.publish(compile_committed_effect(effect.clone()), &mut reconciled),
    )
    .await
    .expect("the exact one-second production timeout bounds the foreign callback");
    assert!(matches!(first, Ok(EndpointDisposition::CircuitDisposed)));
    assert_eq!(calls.load(Ordering::Acquire), 1);

    let second = endpoints
        .publish(compile_committed_effect(effect), &mut reconciled)
        .await;
    assert!(matches!(second, Ok(EndpointDisposition::CircuitDisposed)));
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the open circuit cannot spawn or queue another blocking call"
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
}
