use super::super::{
    effect::EffectLimits,
    ingress::{
        BoundedTransaction, RetainedAdmissionBatch, RetainedIngressAttempt, proposal,
        test_support::remote_at_for_foundation,
    },
    plan::{
        AuthorityFault, CommittedRetainedAdmissionBatch, ConcurrentRetainedIngressError, PlanError,
        StalePlan, TxPoolAuthority,
    },
    runtime::{AuthorityRuntime, RetainedIngressBatchFailure, RetainedIngressBatchFailureReason},
    shard::{ConcurrentRemovalProbe, SharedIngressProbePhase},
    state::{AcceptedStatus, DependencyKey, OwnedTx, PreAcceptedSource, ProposalId, RawTxHash},
};
use super::foundation::{
    accept_remote_transaction_with_payload, genesis_snapshot, limits, resolved_payload_with_facts,
    runtime_config,
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

fn ingress_tx(marker: u8) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(Byte32::new([marker; 32]), 0),
            0,
        ))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn ingress_tx_with_cell_dep(marker: u8, dependency: &OutPoint) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(Byte32::new([marker; 32]), 0),
            0,
        ))
        .cell_dep(CellDep::new_builder().out_point(dependency.clone()).build())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn proposal_batch(
    transactions: impl IntoIterator<Item = TransactionView>,
) -> RetainedAdmissionBatch {
    let consensus = ConsensusBuilder::default().build();
    let mut attempts = transactions
        .into_iter()
        .map(|transaction| {
            let transaction = BoundedTransaction::try_new(transaction)
                .expect("proposal fixture transaction is bounded");
            proposal(transaction, &consensus)
        })
        .collect::<VecDeque<_>>();
    let head = attempts
        .pop_front()
        .expect("fixture constructs a non-empty proposal batch");
    RetainedAdmissionBatch::new(head, attempts).expect("fixture batch is homogeneous")
}

fn remote_batch(
    peer: PeerIndex,
    transactions: impl IntoIterator<Item = TransactionView>,
) -> RetainedAdmissionBatch {
    let consensus = ConsensusBuilder::default().build();
    let mut attempts = transactions
        .into_iter()
        .map(|transaction| {
            remote_at_for_foundation(transaction, 0, peer, 100, &consensus)
                .map(RetainedIngressAttempt::Validated)
                .unwrap_or_else(|attempt| attempt)
        })
        .collect::<VecDeque<_>>();
    let head = attempts
        .pop_front()
        .expect("fixture constructs a non-empty remote batch");
    RetainedAdmissionBatch::new(head, attempts).expect("fixture batch is homogeneous")
}

fn commit_retained_small(
    runtime: &AuthorityRuntime,
    batch: RetainedAdmissionBatch,
) -> Result<(usize, bool), ()> {
    runtime
        .commit_retained_ingress_batch(batch)
        .map(|(consumed, remaining, post_commit_fault)| {
            assert_eq!(post_commit_fault, None);
            (consumed, remaining.is_empty())
        })
        .map_err(|_| ())
}

fn compact_retained_failure(
    failure: RetainedIngressBatchFailure,
) -> (RetainedIngressBatchFailureReason, usize) {
    let (reason, batch) = failure.into_parts();
    (reason, batch.len())
}

#[test]
fn uak_interposed_proposal_promotion_is_shared_contention() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the source-race fixture runtime is valid");
    let transaction = ingress_tx(215);
    let hash = RawTxHash(transaction.hash());
    commit_retained_small(
        &runtime,
        remote_batch(PeerIndex::from(215), [transaction.clone()]),
    )
    .expect("the Remote owner commits");
    let version = runtime.with_authority_read_for_foundation(|authority| {
        authority
            .entry(&hash)
            .expect("the Remote owner exists")
            .record()
            .version
    });

    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::AfterRetainedIngressHeadClassification,
            Some(pause),
        );
    });
    let stale = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let stale_transaction = transaction.clone();
        let stale = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(proposal_batch([stale_transaction]))
                .map(|_| ())
                .map_err(compact_retained_failure)
        });
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the first promotion classifies the Remote owner");
        runtime.with_authority_read_for_foundation(|authority| {
            authority.entries_for_reference().set_shared_ingress_probe(
                SharedIngressProbePhase::AfterRetainedIngressHeadClassification,
                None,
            );
        });
        commit_retained_small(&runtime, proposal_batch([transaction]))
            .expect("the second promotion commits first");
        release
            .send(())
            .expect("release the stale promotion compiler");
        stale.join().expect("the stale promotion thread joins")
    });
    let Err((reason, batch_len)) = stale else {
        panic!("the first promotion cannot commit from its obsolete source premise")
    };
    assert!(matches!(
        reason,
        RetainedIngressBatchFailureReason::SharedContention
    ));
    assert_eq!(batch_len, 1);
    runtime.with_authority_read_for_foundation(|authority| {
        let owner = authority.entry(&hash).expect("the Proposal owner remains");
        assert!(owner.record().version > version);
        assert!(matches!(
            owner,
            OwnedTx::PreAccepted(entry)
                if matches!(entry.source, PreAcceptedSource::Proposal { .. })
        ));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_peer_fence_interposition_rejects_the_released_semantic_cut() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the peer-fence fixture runtime is valid");
    let peer = PeerIndex::from(216);
    let transaction = ingress_tx(216);
    let hash = RawTxHash(transaction.hash());
    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::AfterRetainedIngressSemanticCut,
            Some(pause),
        );
    });

    let stale = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let stale = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(remote_batch(peer, [transaction]))
                .map(|_| ())
                .map_err(compact_retained_failure)
        });
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the valid ingress releases its semantic cut");
        runtime.with_authority_read_for_foundation(|authority| {
            authority.entries_for_reference().set_shared_ingress_probe(
                SharedIngressProbePhase::AfterRetainedIngressSemanticCut,
                None,
            );
        });
        let malformed = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .build();
        commit_retained_small(&runtime, remote_batch(peer, [malformed]))
            .expect("the malformed same-peer batch commits its fence first");
        release
            .send(())
            .expect("release the obsolete valid-ingress compiler");
        stale.join().expect("the valid-ingress thread joins")
    });

    let Err((reason, batch_len)) = stale else {
        panic!("the obsolete Absent peer-fence premise cannot commit")
    };
    assert!(matches!(
        reason,
        RetainedIngressBatchFailureReason::SharedContention
    ));
    assert_eq!(batch_len, 1);
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.entry(&hash).is_none());
        assert!(authority.peer_is_banned_for_reference(peer));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_proposal_identity_interposition_rejects_the_released_semantic_cut() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the proposal-identity fixture runtime is valid");
    let transaction = ingress_tx(217);
    let hash = RawTxHash(transaction.hash());
    let proposal = ProposalId(transaction.proposal_short_id());
    let mut alternate = hash.0.as_slice().to_vec();
    alternate[31] ^= 1;
    let alternate = RawTxHash(
        Byte32::from_slice(&alternate).expect("the colliding raw hash remains fixed-size"),
    );
    assert_eq!(
        ProposalId(ckb_types::packed::ProposalShortId::from_tx_hash(
            &alternate.0,
        )),
        proposal
    );

    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::AfterRetainedIngressSemanticCut,
            Some(pause),
        );
    });
    let stale = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let stale = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(proposal_batch([transaction]))
                .map(|_| ())
                .map_err(compact_retained_failure)
        });
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the proposal ingress releases its semantic cut");
        runtime.with_authority_read_for_foundation(|authority| {
            authority.entries_for_reference().set_shared_ingress_probe(
                SharedIngressProbePhase::AfterRetainedIngressSemanticCut,
                None,
            );
            assert_eq!(
                authority
                    .replace_proposal_owner_for_foundation(&proposal, Some(alternate.clone()),),
                None
            );
        });
        release
            .send(())
            .expect("release the obsolete proposal compiler");
        stale.join().expect("the proposal-ingress thread joins")
    });

    let Err((reason, batch_len)) = stale else {
        panic!("the obsolete vacant proposal premise cannot commit")
    };
    assert!(matches!(
        reason,
        RetainedIngressBatchFailureReason::SharedContention
    ));
    assert_eq!(batch_len, 1);
    runtime.with_authority_read_for_foundation(|authority| {
        assert_eq!(
            authority.replace_proposal_owner_for_foundation(&proposal, None),
            Some(alternate)
        );
        assert!(authority.entry(&hash).is_none());
        assert!(authority.primary_projection_consistent());
    });
}

fn runtime_with_authority(authority: TxPoolAuthority) -> AuthorityRuntime {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production retained-ingress runtime is valid");
    runtime.with_authority_for_foundation(|current| *current = authority);
    runtime
}

fn apply_shared_owner_prefix(
    authority: &TxPoolAuthority,
    batch: &RetainedAdmissionBatch,
) -> CommittedRetainedAdmissionBatch {
    authority
        .compile_shared_retained_ingress_batch(batch)
        .expect("the production shared owner prefix compiles")
        .expect("the batch begins with an owner outcome")
        .bind(authority)
        .expect("the generation remains current")
        .apply()
        .expect("the exact shared owner prefix commits")
}

#[test]
fn uak_shared_retained_vacancy_revision_rejects_absent_present_absent_aba() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(210);
    let transaction = ingress_tx(210);
    let hash = RawTxHash(transaction.hash());
    let batch = || remote_batch(peer, [transaction.clone()]);
    let stale = authority
        .compile_shared_retained_ingress_batch(&batch())
        .expect("the old coherent shared plan compiles")
        .expect("the vacant Remote owner uses the shared shape");
    let winner = authority
        .compile_shared_retained_ingress_batch(&batch())
        .expect("the winning coherent shared plan compiles")
        .expect("the same vacancy can be witnessed before either plan commits");

    drop(
        winner
            .bind(&authority)
            .expect("the winning generation remains current")
            .apply()
            .expect("the first exact vacancy prestate commits"),
    );
    let removal = authority
        .prepare_shared_local_removal_for_foundation(&hash)
        .expect("the inserted owner has one canonical removal")
        .expect("the inserted owner is present");
    drop(removal.apply_for_foundation());
    assert!(authority.entry(&hash).is_none());

    assert!(matches!(
        stale
            .bind(&authority)
            .expect("whole-generation identity remains current")
            .apply(),
        Err(ConcurrentRetainedIngressError::Stale)
    ));
    assert!(authority.entry(&hash).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_ingress_resource_occ_stale_leaves_no_scheduler_or_dependency_rows() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(212);
    let first_batch = remote_batch(peer, [ingress_tx(212)]);
    let second_tx = ingress_tx(213);
    let second_hash = RawTxHash(second_tx.hash());
    let second_dependency = DependencyKey::Cell(OutPoint::new(Byte32::new([213; 32]), 0));
    let second_batch = remote_batch(peer, [second_tx.clone()]);
    let first = authority
        .compile_shared_retained_ingress_batch(&first_batch)
        .expect("the first same-peer plan compiles")
        .expect("the first owner uses the shared shape");
    let stale = authority
        .compile_shared_retained_ingress_batch(&second_batch)
        .expect("the second same-peer plan compiles from the same resource cut")
        .expect("the second owner uses the shared shape");

    drop(
        first
            .bind(&authority)
            .expect("the first generation remains current")
            .apply()
            .expect("the first peer-resource prestate commits"),
    );
    assert!(matches!(
        stale
            .bind(&authority)
            .expect("the generation remains current")
            .apply(),
        Err(ConcurrentRetainedIngressError::Stale)
    ));
    assert!(authority.entry(&second_hash).is_none());
    assert_eq!(
        authority.dependency_consumers_for_foundation(&second_dependency),
        None
    );

    let retry = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [second_tx]))
        .expect("the rolled-back owner replans against the new resource cut")
        .expect("rollback left no scheduler or dependency collision");
    drop(
        retry
            .bind(&authority)
            .expect("the retry generation remains current")
            .apply()
            .expect("the retry commits after the stale preparation is discarded"),
    );
    assert!(authority.entry(&second_hash).is_some());
    assert_eq!(
        authority.dependency_consumers_for_foundation(&second_dependency),
        Some(std::collections::BTreeSet::from([second_hash]))
    );
    assert!(authority.primary_projection_consistent());
}

#[tokio::test]
async fn uak_effect_close_cannot_split_a_staged_shared_ingress_prefix() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the close/staged-ingress fixture runtime is valid");
    let transaction = ingress_tx(216);
    let hash = RawTxHash(transaction.hash());
    let peer = PeerIndex::from(216);
    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::ProjectionPreparedBeforeOwnerCut,
            Some(pause),
        );
    });

    let committed = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let commit = scope
            .spawn(move || commit_retained_small(runtime_ref, remote_batch(peer, [transaction])));
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the ingress prefix is staged under the generation read barrier");
        assert!(
            !runtime.generation_write_available_for_foundation(),
            "effect close must not acquire its generation write cut mid-prefix"
        );
        release.send(()).expect("release the staged ingress prefix");
        commit.join().expect("the staged ingress thread joins")
    });
    assert!(committed.is_ok());
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::ProjectionPreparedBeforeOwnerCut,
            None,
        );
    });
    runtime
        .close_effects()
        .await
        .expect("close follows the complete committed prefix");
    assert!(
        runtime
            .commit_retained_ingress_batch(remote_batch(PeerIndex::from(217), [ingress_tx(217)],))
            .is_err(),
        "a closed effect lifecycle cannot start another staged prefix"
    );
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.entry(&hash).is_some());
        assert_eq!(authority.owner_count(), 1);
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test]
async fn uak_generation_replacement_cannot_splice_a_staged_shared_ingress_prefix() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the generation/staged-ingress fixture runtime is valid");
    let transaction = ingress_tx(218);
    let hash = RawTxHash(transaction.hash());
    let peer = PeerIndex::from(218);
    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::ProjectionPreparedBeforeOwnerCut,
            Some(pause),
        );
    });

    let committed = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let commit = scope
            .spawn(move || commit_retained_small(runtime_ref, remote_batch(peer, [transaction])));
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the ingress prefix is staged under the generation read barrier");
        assert!(
            !runtime.generation_write_available_for_foundation(),
            "generation replacement must not acquire a write cut mid-prefix"
        );
        release.send(()).expect("release the staged ingress prefix");
        commit.join().expect("the staged ingress thread joins")
    });
    assert!(committed.is_ok());
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::ProjectionPreparedBeforeOwnerCut,
            None,
        );
        assert!(authority.entry(&hash).is_some());
    });

    runtime
        .clear_pool(genesis_snapshot())
        .await
        .expect("generation replacement follows the complete ingress prefix");
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.entry(&hash).is_none());
        assert_eq!(authority.owner_count(), 0);
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_parent_terminalization_before_shared_child_stage_preserves_owner_free_loss() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the parent-first fixture runtime is valid");
    let parent_input = OutPoint::new(Byte32::new([221; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(221u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction_with_payload(
            authority,
            parent_tx.clone(),
            221,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &parent_tx,
                Vec::new(),
                vec![parent_input],
                Capacity::shannons(1_000),
            ),
        )
    });
    let parent_output = OutPoint::new(parent.0.clone(), 0);
    let key = DependencyKey::Cell(parent_output.clone());
    assert!(
        runtime
            .remove_local_transaction(&parent.0)
            .expect("the parent-first removal commits")
    );
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(
            authority
                .dependency_unindexed_loss_for_foundation(&key)
                .is_some(),
            "a loss with no committed consumer remains in the owner-free fence"
        );
    });

    let child_tx = ingress_tx_with_cell_dep(222, &parent_output);
    let child_hash = RawTxHash(child_tx.hash());
    let Ok((consumed, remaining, post_commit_fault)) =
        runtime.commit_retained_ingress_batch(remote_batch(PeerIndex::from(222), [child_tx]))
    else {
        panic!("the parent-first child shared prefix commits");
    };
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(post_commit_fault, None);
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.entry(&child_hash).is_some());
        assert_eq!(
            authority.dependency_consumers_for_foundation(&key),
            Some(std::collections::BTreeSet::from([child_hash]))
        );
        assert!(
            authority
                .dependency_unindexed_loss_for_foundation(&key)
                .is_some()
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_late_arriving_disjoint_runtime_ingress_overlaps_before_activation() {
    const CUT_ENTRY_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the late-arrival overlap fixture runtime is valid");
    let shared_dependency = OutPoint::new(Byte32::new([88; 32]), 0);
    let (left_marker, right_marker) = runtime.with_authority_read_for_foundation(|authority| {
        let mut selected = None;
        'left: for left_marker in 120u8..150 {
            let left_peer = PeerIndex::from(usize::from(left_marker));
            let Some(left) = authority
                .compile_shared_retained_ingress_batch(&remote_batch(
                    left_peer,
                    [ingress_tx_with_cell_dep(left_marker, &shared_dependency)],
                ))
                .expect("a singleton validated Remote batch compiles")
            else {
                continue;
            };
            for right_marker in (left_marker + 1)..180 {
                let right_peer = PeerIndex::from(usize::from(right_marker));
                let Some(right) = authority
                    .compile_shared_retained_ingress_batch(&remote_batch(
                        right_peer,
                        [ingress_tx_with_cell_dep(right_marker, &shared_dependency)],
                    ))
                    .expect("a second singleton validated Remote batch compiles")
                else {
                    continue;
                };
                if left.is_compatible_with_for_foundation(authority, &right) {
                    selected = Some((left_marker, right_marker));
                    break 'left;
                }
            }
        }
        selected.expect("the fixed layout contains two disjoint retained-ingress cuts")
    });
    let left_peer = PeerIndex::from(usize::from(left_marker));
    let right_peer = PeerIndex::from(usize::from(right_marker));
    let left_hash = RawTxHash(ingress_tx_with_cell_dep(left_marker, &shared_dependency).hash());
    let right_hash = RawTxHash(ingress_tx_with_cell_dep(right_marker, &shared_dependency).hash());
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::FinalCutBeforeActivation,
            Some(probe),
        );
    });
    let committed = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let left_dependency = shared_dependency.clone();
        let left = scope.spawn(move || {
            commit_retained_small(
                runtime_ref,
                remote_batch(
                    left_peer,
                    [ingress_tx_with_cell_dep(left_marker, &left_dependency)],
                ),
            )
        });
        entered
            .recv_timeout(CUT_ENTRY_TIMEOUT)
            .expect("the first runtime ingress reached its final owner cut");
        let runtime_ref = &runtime;
        let right_dependency = shared_dependency.clone();
        let right = scope.spawn(move || {
            commit_retained_small(
                runtime_ref,
                remote_batch(
                    right_peer,
                    [ingress_tx_with_cell_dep(right_marker, &right_dependency)],
                ),
            )
        });
        let overlapped = entered.recv_timeout(CUT_ENTRY_TIMEOUT).is_ok();
        release.send(()).expect("release the first live cut");
        if !overlapped {
            entered
                .recv_timeout(CUT_ENTRY_TIMEOUT)
                .expect("the second runtime ingress reaches its cut after the first releases");
        }
        release.send(()).expect("release the second live cut");
        let committed = [
            left.join().expect("left runtime ingress thread joins"),
            right.join().expect("right runtime ingress thread joins"),
        ];
        assert!(
            overlapped,
            "a late-arriving disjoint runtime ingress must enter before the first owner cut releases"
        );
        committed
    });
    assert!(
        committed
            .into_iter()
            .all(|result| { matches!(result, Ok((1, true))) })
    );
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::FinalCutBeforeActivation, None);
        assert!(authority.entry(&left_hash).is_some());
        assert!(authority.entry(&right_hash).is_some());
        let dependency_consumers = authority
            .dependency_consumers_for_foundation(&DependencyKey::Cell(shared_dependency))
            .expect("the shared read-only cell-dep has both committed consumers");
        assert_eq!(dependency_consumers.len(), 2);
        assert_eq!(
            authority
                .preaccepted_for_peer_for_reference(left_peer)
                .len(),
            1
        );
        assert_eq!(
            authority
                .preaccepted_for_peer_for_reference(right_peer)
                .len(),
            1
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_shared_owner_occ_loss_is_typed_backpressure_without_outer_write_fallback() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the shared-contention fixture runtime is valid");
    let transaction = ingress_tx(185);
    let hash = RawTxHash(transaction.hash());
    assert!(matches!(
        commit_retained_small(
            &runtime,
            remote_batch(PeerIndex::from(185), [transaction.clone()]),
        ),
        Ok((1, true))
    ));

    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::ProjectionPreparedBeforeOwnerCut,
            Some(pause),
        );
    });
    let (reason, batch_len) = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let commit = scope.spawn(move || {
            let failure = runtime_ref
                .commit_retained_ingress_batch(proposal_batch([transaction]))
                .expect_err("ordinary shared contention must not enter the outer write fallback");
            let (reason, batch) = failure.into_parts();
            (reason, batch.len())
        });
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the promotion stages before its final owner cut");
        runtime.with_authority_read_for_foundation(|authority| {
            authority.cycle_owner_row_during_shared_plan_for_foundation(&hash);
        });
        release
            .send(())
            .expect("release the stale shared promotion");
        commit.join().expect("the contended promotion thread joins")
    });
    assert!(matches!(
        reason,
        RetainedIngressBatchFailureReason::SharedContention
    ));
    assert_eq!(batch_len, 1);
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::ProjectionPreparedBeforeOwnerCut,
            None,
        );
        assert!(matches!(
            authority.entry(&hash),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.source, PreAcceptedSource::Remote(_))
        ));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_malformed_remote_batch_revokes_the_peer_before_any_batch_owner_survives() {
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(43);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let resident = ingress_tx(8);
    let resident_hash = RawTxHash(resident.hash());
    let resident = remote_at_for_foundation(resident, 0, peer, 100, &consensus)
        .expect("resident fixture validates");
    drop(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(resident))
            .expect("resident fixture commits through the production shared route"),
    );

    let accepted_tx = ingress_tx(9);
    let accepted_hash = accept_remote_transaction_with_payload(
        &mut authority,
        accepted_tx.clone(),
        9,
        super::super::state::AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &accepted_tx,
            Vec::new(),
            accepted_tx.input_pts_iter().collect(),
            Capacity::shannons(1),
        ),
    );

    let fresh = ingress_tx(10);
    let fresh_hash = RawTxHash(fresh.hash());
    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    let batch = remote_batch(peer, [fresh, malformed]);
    let committed = authority
        .plan_shared_peer_revocation(&batch)
        .expect("malformed cohort plans one peer revocation")
        .expect("the production shared malformed route is selected")
        .apply()
        .expect("the exact malformed peer cohort commits");
    assert_eq!(committed.consumed(), 2);
    drop(committed);

    assert!(authority.entry(&resident_hash).is_none());
    assert!(authority.entry(&fresh_hash).is_none());
    assert!(authority.entry(&accepted_hash).is_some());
    assert!(authority.peer_is_banned_for_reference(peer));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_prepared_shared_peer_revocation_excludes_same_key_event_and_drops_cleanly() {
    let peer = PeerIndex::from(143);
    let authority = TxPoolAuthority::for_foundation(limits());
    let resident = ingress_tx(108);
    let resident_hash = RawTxHash(resident.hash());
    let dependency = DependencyKey::Cell(
        resident
            .input_pts_iter()
            .next()
            .expect("the resident fixture has one dependency input"),
    );
    let resident_batch = remote_batch(peer, [resident]);
    let committed = apply_shared_owner_prefix(&authority, &resident_batch);
    assert_eq!(committed.consumed(), 1);
    drop(committed);
    assert_eq!(
        authority.dependency_consumers_for_foundation(&dependency),
        Some(std::collections::BTreeSet::from([resident_hash.clone()])),
    );
    let dependency_event = authority
        .plan_dependency_loss_for_foundation(vec![dependency.clone()])
        .expect("the same-key loss event plans")
        .expect("the resident consumer gives the loss event one transition");
    assert!(
        dependency_event.dependency_gate_cut_available_for_foundation(),
        "the exact event gate cut is initially available"
    );
    let before = authority.normalized_snapshot();
    let before_effect = authority.effect_observation_for_foundation();
    let before_rows = authority.peer_ingress_row_count_for_reference();

    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    let batch = remote_batch(peer, [malformed]);
    let prepared = authority
        .plan_shared_peer_revocation(&batch)
        .expect("the exact malformed cohort stages")
        .expect("the shared peer-revocation route is selected");
    assert!(authority.peer_fence_hidden_for_reference(peer));
    assert!(authority.entry(&resident_hash).is_some());
    assert_ne!(authority.effect_observation_for_foundation(), before_effect);
    assert!(
        !dependency_event.dependency_gate_cut_available_for_foundation(),
        "the prepared removal exclusively owns the same dependency-key gate"
    );
    drop(prepared);

    let after = authority.normalized_snapshot();
    assert!(after.equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1));
    assert!(
        dependency_event.dependency_gate_cut_available_for_foundation(),
        "dropping the prepared removal releases its exact dependency gates"
    );
    assert_eq!(authority.effect_observation_for_foundation(), before_effect);
    assert_eq!(
        authority.peer_ingress_row_count_for_reference(),
        before_rows
    );
    assert!(!authority.peer_fence_hidden_for_reference(peer));
    assert!(!authority.peer_is_banned_for_reference(peer));
    assert!(authority.entry(&resident_hash).is_some());
    assert_eq!(
        authority.preaccepted_for_peer_for_reference(peer),
        vec![resident_hash.clone()]
    );
    drop(
        dependency_event
            .apply()
            .expect("the same-key event commits after the prepared removal drops"),
    );
    assert_eq!(
        authority.dependency_consumers_for_foundation(&dependency),
        Some(std::collections::BTreeSet::from([resident_hash])),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_active_shared_peer_fence_survives_fresh_generation_replacement() {
    let peer = PeerIndex::from(146);
    let other_peer = PeerIndex::from(246);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let other = ingress_tx(246);
    let other_hash = RawTxHash(other.hash());
    let other_batch = remote_batch(other_peer, [other]);
    drop(apply_shared_owner_prefix(&authority, &other_batch));
    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    let batch = remote_batch(peer, [malformed]);
    let committed = authority
        .plan_shared_peer_revocation(&batch)
        .expect("the exact malformed cohort stages")
        .expect("the shared peer-revocation route is selected")
        .apply()
        .expect("the peer fence commits");
    drop(committed);
    assert!(authority.peer_is_banned_for_reference(peer));
    assert!(authority.entry(&other_hash).is_some());
    let live_layout = authority.entries_for_reference().clone();
    let swaps_before = live_layout.generation_payload_swaps_for_test();

    let retirement = authority
        .plan_clear_pool(Byte32::new([146; 32]))
        .expect("fresh-generation replacement prepares one empty payload carrier")
        .apply();
    drop(retirement);
    assert!(
        authority
            .entries_for_reference()
            .same_layout_for_test(&live_layout)
    );
    assert!(authority.peer_is_banned_for_reference(peer));
    assert!(!authority.peer_fence_hidden_for_reference(peer));
    assert!(authority.entry(&other_hash).is_none());
    assert!(
        authority
            .preaccepted_for_peer_for_reference(other_peer)
            .is_empty()
    );
    assert_eq!(
        authority
            .entries_for_reference()
            .generation_payload_swaps_for_test(),
        swaps_before + crate::authority::shard::AUTHORITY_SHARD_COUNT
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_expired_victim_fence_is_removed_without_deleting_its_new_owner() {
    let victim = PeerIndex::from(147);
    let newcomer = PeerIndex::from(148);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.set_peer_ban_limit_for_foundation(1);
    let expired_observation = Instant::now()
        .checked_sub(Duration::from_secs(
            crate::constants::MALFORMED_TX_BAN_SECONDS + 1,
        ))
        .expect("the monotonic test clock represents one expired lease");
    let retirement = authority
        .plan_peer_revocation_at_for_foundation(victim, expired_observation)
        .expect("the fixture installs one expired bounded fence")
        .apply();
    drop(retirement);
    assert!(!authority.peer_is_banned_for_reference(victim));

    let resident = ingress_tx(109);
    let resident_hash = RawTxHash(resident.hash());
    let resident_batch = remote_batch(victim, [resident]);
    let committed = apply_shared_owner_prefix(&authority, &resident_batch);
    assert_eq!(committed.consumed(), 1);
    drop(committed);

    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    let batch = remote_batch(newcomer, [malformed]);
    let committed = authority
        .plan_shared_peer_revocation(&batch)
        .expect("the new peer reuses the expired slot")
        .expect("the production shared malformed route is selected")
        .apply()
        .expect("the exact expired-victim transition commits");
    drop(committed);

    assert!(authority.entry(&resident_hash).is_some());
    assert_eq!(
        authority.preaccepted_for_peer_for_reference(victim),
        vec![resident_hash]
    );
    assert!(!authority.peer_is_banned_for_reference(victim));
    assert!(authority.peer_is_banned_for_reference(newcomer));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_fence_only_staged_expired_replacements_obey_the_two_times_capacity_row_bound() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.set_peer_ban_limit_for_foundation(2);
    let expired_observation = Instant::now()
        .checked_sub(Duration::from_secs(
            crate::constants::MALFORMED_TX_BAN_SECONDS + 1,
        ))
        .expect("the monotonic test clock represents one expired lease");
    for peer in [PeerIndex::from(149), PeerIndex::from(150)] {
        let retirement = authority
            .plan_peer_revocation_at_for_foundation(peer, expired_observation)
            .expect("the fixture fills one expired bounded slot")
            .apply();
        drop(retirement);
    }
    assert_eq!(authority.peer_ingress_row_count_for_reference(), 2);

    let malformed = |marker| {
        TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(marker))
            .build()
    };
    let left_batch = remote_batch(PeerIndex::from(151), [malformed(151)]);
    let right_batch = remote_batch(PeerIndex::from(152), [malformed(152)]);
    let left = authority
        .plan_shared_peer_revocation(&left_batch)
        .expect("the first expired victim stages")
        .expect("the first malformed route is selected");
    let right = authority
        .plan_shared_peer_revocation(&right_batch)
        .expect("the disjoint expired victim stages")
        .expect("the second malformed route is selected");
    assert_eq!(
        authority.peer_ingress_row_count_for_reference(),
        4,
        "two old rollback rows plus two hidden targets attain the exact 2C transient bound"
    );
    drop(right);
    drop(left);
    assert_eq!(authority.peer_ingress_row_count_for_reference(), 2);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_structural_bank_begin_failure_restores_the_hidden_peer_row() {
    let peer = PeerIndex::from(153);
    let authority = TxPoolAuthority::for_foundation(limits());
    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(153))
        .build();
    let batch = remote_batch(peer, [malformed]);
    let prepared = authority
        .plan_shared_peer_revocation(&batch)
        .expect("the exact malformed cohort stages")
        .expect("the production shared malformed route is selected");
    let stage_id = prepared.peer_fence_stage_id_for_foundation();
    assert!(authority.peer_fence_hidden_for_reference(peer));
    assert!(authority.invalidate_peer_ban_stage_for_test(stage_id));
    assert!(matches!(
        prepared.apply().map_err(|failure| failure.into_parts().0),
        Err(ConcurrentRetainedIngressError::Fault(
            AuthorityFault::MembershipProjection
        ))
    ));
    assert!(authority.resources().capacity_faulted_for_foundation());
    assert!(!authority.peer_fence_hidden_for_reference(peer));
    assert!(!authority.peer_is_banned_for_reference(peer));
    assert_eq!(authority.peer_ingress_row_count_for_reference(), 0);

    let next_peer = PeerIndex::from(154);
    let next_malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(154))
        .build();
    let next = remote_batch(next_peer, [next_malformed]);
    assert!(matches!(
        authority.plan_shared_peer_revocation(&next),
        Err(PlanError::Fault(AuthorityFault::MembershipProjection))
    ));
}

#[test]
fn uak_live_peer_ban_stage_is_retryable_and_recovers_on_drop() {
    let peer = PeerIndex::from(155);
    let authority = TxPoolAuthority::for_foundation(limits());
    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(155))
        .build();
    let batch = remote_batch(peer, [malformed]);
    let staged = authority
        .plan_shared_peer_revocation(&batch)
        .expect("the first exact peer-ban slot stages")
        .expect("the malformed route is selected");

    assert!(matches!(
        authority.plan_shared_peer_revocation(&batch),
        Err(PlanError::Stale(StalePlan::Version))
    ));
    drop(staged);
    assert!(
        authority
            .plan_shared_peer_revocation(&batch)
            .expect("dropping the live reservation restores retryability")
            .is_some()
    );
}

#[test]
fn uak_shared_retained_effect_prefix_commits_only_the_bounded_prefix() {
    let effect_limits =
        EffectLimits::for_foundation().with_remote_effects_per_batch_for_foundation(2);
    let authority = TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("fixture effect limits are valid");
    let peer = PeerIndex::from(144);
    let invalid = |marker| {
        ingress_tx(marker)
            .as_advanced_builder()
            .version(1u32)
            .build()
    };
    let prepared = authority
        .plan_shared_retained_effect_prefix(&remote_batch(
            peer,
            [invalid(111), invalid(112), invalid(113)],
        ))
        .expect("the bounded shared effect prefix plans")
        .expect("no item in the prefix owns a transaction row");
    let committed = prepared.apply();
    assert_eq!(committed.consumed(), 2);
    drop(committed);

    let effects = authority.effect_observation_for_foundation();
    assert_eq!(effects.queued.len(), 1);
    assert_eq!(effects.pending_recent_rejects, 2);
    assert_eq!(authority.owner_count(), 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_retained_noop_prefix_has_no_apply_or_clock_advance() {
    let batch = RetainedAdmissionBatch::new(
        RetainedIngressAttempt::ProposalUnavailable,
        VecDeque::from([RetainedIngressAttempt::ProposalUnavailable]),
    )
    .expect("the unavailable proposal batch is homogeneous");
    let authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    let committed = authority
        .plan_shared_retained_effect_prefix(&batch)
        .expect("the shared no-op prefix plans")
        .expect("the complete batch has no owner outcome")
        .apply();

    assert_eq!(committed.consumed(), 2);
    drop(committed);
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_retained_ingress_batch_effect_cut_keeps_only_prior_owner_mutations() {
    let effect_limits =
        EffectLimits::for_foundation().with_remote_effects_per_batch_for_foundation(1);
    let authority = TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("fixture effect limits are valid");
    let runtime = runtime_with_authority(authority);
    let peer = PeerIndex::from(45);
    let transactions = [
        ingress_tx(17),
        ingress_tx(18),
        ingress_tx(19),
        ingress_tx(20),
    ];
    let (owner_count, mut remaining, fault) = runtime
        .commit_retained_ingress_batch(remote_batch(peer, transactions))
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the production owner prefix commits")
        });
    assert_eq!(owner_count, 2);
    assert_eq!(fault, None);
    runtime.with_authority_read_for_foundation(|authority| {
        assert_eq!(authority.owner_count(), 2);
        assert_eq!(authority.preaccepted_for_peer_for_reference(peer).len(), 2);
    });
    let head = remaining
        .pop_front()
        .expect("the first pressure item starts the exact suffix");
    let suffix = RetainedAdmissionBatch::new(head, remaining)
        .expect("the pressure suffix remains homogeneous");
    let (effect_count, remaining, fault) = runtime
        .commit_retained_ingress_batch(suffix)
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the first bounded pressure effect commits")
        });
    assert_eq!(effect_count, 1);
    assert_eq!(remaining.len(), 1);
    assert_eq!(fault, None);
    runtime.with_authority_read_for_foundation(|authority| {
        assert_eq!(authority.owner_count(), 2);
        assert!(authority.primary_projection_consistent());
    });
}
