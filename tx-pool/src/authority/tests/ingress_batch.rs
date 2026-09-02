use super::super::{
    effect::EffectLimits,
    ingress::{
        BoundedTransaction, RetainedAdmissionBatch, RetainedIngressAttempt, proposal,
        test_support::remote_at_for_foundation,
    },
    plan::{
        AuthorityFault, CommittedRetainedAdmissionBatch, ConcurrentRetainedIngressError, PlanError,
        TxPoolAuthority, test_support::RetainedAdmissionDisposition,
    },
    runtime::{AuthorityRuntime, RetainedIngressBatchFailureReason},
    shard::{ConcurrentRemovalProbe, SharedIngressProbePhase},
    state::{
        AcceptedStatus, ApplySequence, Arrival, DependencyKey, EntryVersion, OwnedTx,
        PoolGeneration, PreAcceptedPhase, PreAcceptedSource, QueuedWork, RawTxHash, RemoteDeadline,
        RemoteResidencyLease, ValidatedAdmission, WorkPermit,
    },
    work::CheckedOutWork,
};
use super::foundation::{
    accept_remote_transaction_with_payload, admit_remote_until, apply_plan, genesis_snapshot,
    limits, missing_keys, owner_version, resolved_payload_with_facts, runtime_config,
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
    num::NonZeroUsize,
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

fn accepted_parent_and_child(
    authority: &mut TxPoolAuthority,
    parent_marker: u8,
    child_marker: u8,
) -> (RawTxHash, OutPoint, TransactionView) {
    let parent_input = OutPoint::new(Byte32::new([parent_marker; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(u32::from(parent_marker))
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept_remote_transaction_with_payload(
        authority,
        parent_tx.clone(),
        usize::from(parent_marker),
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            vec![parent_input],
            Capacity::shannons(1_000),
        ),
    );
    let parent_output = OutPoint::new(parent.0.clone(), 0);
    let child_tx = ingress_tx_with_cell_dep(child_marker, &parent_output);
    (parent, parent_output, child_tx)
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

fn commit_all_retained(runtime: &AuthorityRuntime, mut batch: RetainedAdmissionBatch) -> usize {
    let mut total = 0usize;
    loop {
        let (consumed, mut remaining, fault) = runtime
            .commit_retained_ingress_batch(batch)
            .unwrap_or_else(|failure| {
                drop(failure);
                panic!("the production retained-ingress prefix commits")
            });
        assert_eq!(fault, None);
        total = total
            .checked_add(consumed)
            .expect("the bounded fixture count is representable");
        let Some(head) = remaining.pop_front() else {
            return total;
        };
        batch = RetainedAdmissionBatch::new(head, remaining)
            .expect("the exact production suffix remains homogeneous");
    }
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
fn uak_remote_expiry_rejects_an_interposed_earlier_head_and_rolls_back_exact_capacity() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let selected = admit_remote_until(&mut authority, 2_301, 301, 10);
    let reticketed = admit_remote_until(&mut authority, 2_302, 302, 30);
    let compiled = authority
        .plan_remote_expiry(
            RemoteDeadline(20),
            NonZeroUsize::new(1).expect("one is non-zero"),
        )
        .expect("the exact prefix plans")
        .expect("the first Remote owner is due");

    authority.reticket_remote_deadline_for_foundation(&reticketed, RemoteDeadline(5));
    let before = authority.normalized_snapshot();
    let before_resources = authority.resources().snapshot();
    let before_effect = authority.effect_observation_for_foundation();
    let prepared = compiled
        .bind(&authority)
        .expect("reticketing an unrelated owner changes no staged projection row");
    assert_ne!(authority.effect_observation_for_foundation(), before_effect);

    let failure = match prepared.apply() {
        Ok(_) => panic!("the final mixed cut rejects the newly earlier deadline head"),
        Err(failure) => failure,
    };
    let (error, effect_wake) = failure.into_parts();
    assert!(matches!(error, ConcurrentRetainedIngressError::Stale));
    assert!(
        effect_wake.is_some_and(|wake| wake.capacity_released()),
        "stale Apply returns the exact staged-effect capacity edge"
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 0)
    );
    assert_eq!(authority.resources().snapshot(), before_resources);
    assert_eq!(authority.effect_observation_for_foundation(), before_effect);
    assert!(authority.entry(&selected).is_some());
    assert!(authority.entry(&reticketed).is_some());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_remote_expiry_bind_rejects_same_key_version_change_without_mutation() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let selected = admit_remote_until(&mut authority, 2_303, 303, 10);
    let compiled = authority
        .plan_remote_expiry(
            RemoteDeadline(10),
            NonZeroUsize::new(1).expect("one is non-zero"),
        )
        .expect("the exact prefix plans")
        .expect("the Remote owner is due");
    let previous_version = owner_version(&authority, &selected);
    let checkout = authority
        .plan_checkout_for_foundation(&selected, previous_version, WorkPermit::ResolveOnly)
        .expect("the same owner advances to active work")
        .apply();
    let CheckedOutWork::Resolve(_work) = checkout.into_work() else {
        panic!("resolve-only checkout returns Resolve work");
    };
    assert_ne!(owner_version(&authority, &selected), previous_version);
    let before = authority.normalized_snapshot();
    let before_resources = authority.resources().snapshot();
    let before_effect = authority.effect_observation_for_foundation();

    assert!(matches!(
        compiled.bind(&authority),
        Err(PlanError::Stale(_))
    ));
    assert_eq!(authority.normalized_snapshot(), before);
    assert_eq!(authority.resources().snapshot(), before_resources);
    assert_eq!(authority.effect_observation_for_foundation(), before_effect);
    assert!(authority.entry(&selected).is_some());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_new_remote_ingress_rejects_a_stale_duplicate_before_any_projection_changes() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(211);
    let first = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [ingress_tx(211)]))
        .expect("the first coherent shared plan compiles")
        .expect("a new validated Remote owner uses the shared shape");
    let stale = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [ingress_tx(211)]))
        .expect("the second coherent shared plan compiles")
        .expect("the duplicate is not visible before either plan applies");

    drop(
        first
            .bind(&authority)
            .expect("the first generation remains current")
            .apply()
            .expect("the first exact prestate commits"),
    );
    let stale = stale
        .bind(&authority)
        .expect("the generation remains current")
        .apply();
    assert!(matches!(stale, Err(ConcurrentRetainedIngressError::Stale)));
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.preaccepted_for_peer_for_reference(peer).len(), 1);
    assert!(authority.primary_projection_consistent());
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
fn uak_shared_existing_remote_proposal_promotion_changes_version_and_owner_atomically() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(212);
    let transaction = ingress_tx(212);
    let hash = RawTxHash(transaction.hash());
    let remote = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [transaction.clone()]))
        .expect("the initial Remote owner compiles")
        .expect("the initial Remote owner has the shared shape");
    drop(
        remote
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the initial Remote owner commits"),
    );
    let remote_version = authority
        .entry(&hash)
        .expect("the Remote owner exists")
        .record()
        .version;

    let promotion = authority
        .compile_shared_retained_ingress_batch(&proposal_batch([transaction]))
        .expect("the Proposal promotion compiles")
        .expect("the existing Remote owner has the shared promotion shape");
    drop(
        promotion
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the exact promotion prestate commits"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Proposal { .. })
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                && entry.record.version != remote_version
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_existing_owner_promotion_rejects_present_absent_present_aba() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(217);
    let transaction = ingress_tx(217);
    let hash = RawTxHash(transaction.hash());
    let remote = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [transaction.clone()]))
        .expect("the Remote owner compiles")
        .expect("the Remote owner has the shared shape");
    drop(
        remote
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the Remote owner commits"),
    );
    let stale = authority
        .compile_shared_retained_ingress_batch(&proposal_batch([transaction]))
        .expect("the old Proposal promotion compiles")
        .expect("the old Proposal promotion has the shared shape");
    authority.cycle_owner_row_during_shared_plan_for_foundation(&hash);
    assert!(matches!(
        stale
            .bind(&authority)
            .expect("the generation remains current")
            .apply(),
        Err(ConcurrentRetainedIngressError::Stale)
    ));
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Remote(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_remote_payload_variant_promotion_moves_the_new_witness_without_fallback() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(213);
    let raw = ingress_tx(213);
    let remote = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
        .build();
    let proposal = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
        .build();
    let hash = RawTxHash(raw.hash());
    let remote_plan = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [remote]))
        .expect("the initial Remote witness compiles")
        .expect("the initial Remote witness has the shared shape");
    drop(
        remote_plan
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the initial Remote witness commits"),
    );
    let remote_version = authority
        .entry(&hash)
        .expect("the Remote owner exists")
        .record()
        .version;

    let promotion = authority
        .compile_shared_retained_ingress_batch(&proposal_batch([proposal.clone()]))
        .expect("the payload-variant promotion compiles")
        .expect("the payload-variant promotion has the shared shape");
    drop(
        promotion
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the payload-variant promotion commits"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if entry.record.tx.hash() == proposal.hash()
                && entry.record.tx.witness_hash() == proposal.witness_hash()
                && entry.record.version != remote_version
                && matches!(entry.source, PreAcceptedSource::Proposal { .. })
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_waiting_remote_proposal_promotion_reactivates_resolve_atomically() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = ingress_tx(214);
    let admission = ValidatedAdmission::remote_with_lease(
        transaction.clone(),
        RemoteResidencyLease::new(PeerIndex::from(715usize), RemoteDeadline(10)),
        0,
    )
    .expect("the Remote lease fixture is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("the Remote lease fixture plans"),
    );
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("remote resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .missing(missing_keys())
                    .expect("the missing-parent fixture is bounded"),
            )
            .expect("Remote missing evidence enters the waiting level"),
    );
    let waiting_version = owner_version(&authority, &hash);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
    ));

    let promotion = authority
        .compile_shared_retained_ingress_batch(&proposal_batch([transaction]))
        .expect("the Waiting-to-Proposal promotion compiles")
        .expect("the Waiting-to-Proposal promotion has the shared shape");
    drop(
        promotion
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the exact Waiting promotion commits"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Proposal { .. })
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                && entry.record.version != waiting_version
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_ingress_resource_occ_stale_rolls_back_hidden_scheduler_and_dependency_rows() {
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
            .expect("the retry commits after exact staged-row rollback"),
    );
    assert!(authority.entry(&second_hash).is_some());
    assert_eq!(
        authority.dependency_consumers_for_foundation(&second_dependency),
        Some(std::collections::BTreeSet::from([second_hash]))
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_stale_proposal_promotion_restores_the_exact_remote_scheduler_projection() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(215);
    let promoted_tx = ingress_tx(215);
    let promoted_hash = RawTxHash(promoted_tx.hash());
    let remote = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [promoted_tx.clone()]))
        .expect("the Remote owner compiles")
        .expect("the Remote owner has the shared shape");
    drop(
        remote
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the Remote owner commits"),
    );
    let remote_version = authority
        .entry(&promoted_hash)
        .expect("the Remote owner exists")
        .record()
        .version;
    let stale = authority
        .compile_shared_retained_ingress_batch(&proposal_batch([promoted_tx]))
        .expect("the Proposal promotion compiles")
        .expect("the Proposal promotion has the shared shape");

    let contender = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [ingress_tx(216)]))
        .expect("the same-peer contender compiles")
        .expect("the same-peer contender has the shared shape");
    drop(
        contender
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the contender changes the resource prestate"),
    );
    assert!(matches!(
        stale
            .bind(&authority)
            .expect("the generation remains current")
            .apply(),
        Err(ConcurrentRetainedIngressError::Stale)
    ));
    assert!(matches!(
        authority.entry(&promoted_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Remote(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                && entry.record.version == remote_version
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_ingress_stages_owner_scheduler_and_dependency_as_one_invisible_prefix() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the staged visibility fixture runtime is valid");
    let dependency = OutPoint::new(Byte32::new([214; 32]), 0);
    let transactions = [
        ingress_tx_with_cell_dep(215, &dependency),
        ingress_tx_with_cell_dep(216, &dependency),
    ];
    let hashes = transactions
        .iter()
        .map(|transaction| RawTxHash(transaction.hash()))
        .collect::<Vec<_>>();
    let key = DependencyKey::Cell(dependency);
    let peer = PeerIndex::from(215);
    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, Some(pause));
    });

    let committed = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let commit = scope
            .spawn(move || commit_retained_small(runtime_ref, remote_batch(peer, transactions)));
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("dependency and scheduler storage are staged before owner mutation");
        runtime.with_authority_read_for_foundation(|authority| {
            assert!(hashes.iter().all(|hash| authority.entry(hash).is_none()));
            assert_eq!(authority.dependency_consumers_for_foundation(&key), None);
            assert_eq!(authority.scheduler_resolve_head_for_foundation(), None);
        });
        release
            .send(())
            .expect("release the aggregate staged-ingress prefix");
        commit.join().expect("the staged ingress thread joins")
    });
    let Ok((consumed, remaining_is_empty)) = committed else {
        panic!("the staged ingress prefix commits as one production transition");
    };
    assert_eq!(consumed, 2);
    assert!(remaining_is_empty);

    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, None);
        assert!(hashes.iter().all(|hash| authority.entry(hash).is_some()));
        assert_eq!(
            authority.dependency_consumers_for_foundation(&key),
            Some(hashes.into_iter().collect())
        );
        assert!(authority.scheduler_resolve_head_for_foundation().is_some());
        assert!(authority.primary_projection_consistent());
    });
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
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, Some(pause));
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
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, None);
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
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, Some(pause));
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
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, None);
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
fn uak_scheduler_sealed_retained_dependency_has_no_final_shard_support() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let peer = PeerIndex::from(217);
    let dependency = OutPoint::new(Byte32::new([217; 32]), 0);
    let transaction = ingress_tx_with_cell_dep(218, &dependency);

    let insertion = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [transaction.clone()]))
        .expect("the retained insertion compiles")
        .expect("the retained insertion has the shared owner shape")
        .bind(&authority)
        .expect("the insertion generation is current")
        .dependency_final_support_masks_for_foundation()
        .expect("the retained insertion seals its exact edge receipts");
    assert_eq!(insertion, (0, 0));

    let committed = authority
        .compile_shared_retained_ingress_batch(&remote_batch(peer, [transaction.clone()]))
        .expect("the insertion recompiles after staged rollback")
        .expect("the insertion remains on the shared route")
        .bind(&authority)
        .expect("the insertion generation remains current")
        .apply()
        .expect("the insertion commits");
    drop(committed);

    let promotion = authority
        .compile_shared_retained_ingress_batch(&proposal_batch([transaction]))
        .expect("the source-only promotion compiles")
        .expect("the source-only promotion has the shared owner shape")
        .bind(&authority)
        .expect("the promotion generation is current")
        .dependency_final_support_masks_for_foundation()
        .expect("the source-only promotion seals its unchanged dependency receipt");
    assert_eq!(promotion, (0, 0));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_dependency_stage_cut_linearizes_with_parent_terminalization_before_rows_exist() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_input = OutPoint::new(Byte32::new([219; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(219u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        219,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            vec![parent_input],
            Capacity::shannons(1_000),
        ),
    );
    let parent_plan = authority
        .prepare_shared_local_removal_for_foundation(&parent)
        .expect("the parent removal plans before child staging")
        .expect("the parent remains present");
    let parent_support = parent_plan.physical_write_support_for_foundation();
    drop(parent_plan);
    let child_tx = ingress_tx_with_cell_dep(220, &OutPoint::new(parent.0.clone(), 0));
    let child = authority
        .compile_shared_retained_ingress_batch(&remote_batch(PeerIndex::from(220), [child_tx]))
        .expect("the child shared-ingress plan compiles")
        .expect("the all-new Remote child uses the shared shape")
        .bind(&authority)
        .expect("the child binds to the live generation");
    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    authority.entries_for_reference().set_shared_ingress_probe(
        SharedIngressProbePhase::DependencyStageBeforeRows,
        Some(pause),
    );

    let committed = std::thread::scope(|scope| {
        let commit = scope.spawn(move || child.apply());
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the dependency stage owns its exact row cut before insertion");
        assert!(
            authority
                .entries_for_reference()
                .try_write_cut(parent_support)
                .is_none(),
            "the pre-row stage and parent terminalization must share one physical linearization cut"
        );
        release.send(()).expect("release the dependency stage cut");
        commit.join().expect("the shared child thread joins")
    });
    assert!(committed.is_ok());
    authority
        .entries_for_reference()
        .set_shared_ingress_probe(SharedIngressProbePhase::DependencyStageBeforeRows, None);
    assert!(authority.primary_projection_consistent());
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
            let left_support = left.physical_write_support_for_foundation(authority);
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
                let right_support = right.physical_write_support_for_foundation(authority);
                if left_support.is_disjoint(right_support) {
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
fn uak_same_owner_late_arrival_cannot_enter_a_competing_final_cut() {
    const START_TIMEOUT: Duration = Duration::from_secs(5);
    const CONFLICT_OBSERVATION: Duration = Duration::from_secs(1);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the same-owner negative-control runtime is valid");
    let transaction = ingress_tx(181);
    let hash = RawTxHash(transaction.hash());
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::FinalCutBeforeActivation,
            Some(probe),
        );
    });
    let (started_tx, started_rx) = std::sync::mpsc::channel();

    let committed = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let first_tx = transaction.clone();
        let first = scope.spawn(move || {
            commit_retained_small(runtime_ref, remote_batch(PeerIndex::from(181), [first_tx]))
        });
        entered
            .recv_timeout(START_TIMEOUT)
            .expect("the first owner insertion holds its final cut");

        let runtime_ref = &runtime;
        let second = scope.spawn(move || {
            started_tx
                .send(())
                .expect("the competing runtime request announces its start");
            commit_retained_small(
                runtime_ref,
                remote_batch(PeerIndex::from(182), [transaction]),
            )
        });
        started_rx
            .recv_timeout(START_TIMEOUT)
            .expect("the competing runtime request has started");
        assert!(
            entered.recv_timeout(CONFLICT_OBSERVATION).is_err(),
            "the same owner must not enter a second final insertion cut"
        );
        release.send(()).expect("release the sole owner cut");
        [
            first.join().expect("the first runtime request joins"),
            second.join().expect("the competing runtime request joins"),
        ]
    });
    assert!(committed.into_iter().all(|result| result.is_ok()));
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::FinalCutBeforeActivation, None);
        assert!(authority.entry(&hash).is_some());
        assert_eq!(authority.owner_count(), 1);
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_same_owner_commit_between_absence_and_resource_receipt_is_stale_not_fault() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the resource-receipt OCC fixture runtime is valid");
    let transaction = ingress_tx(183);
    let hash = RawTxHash(transaction.hash());
    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::BeforeInsertionResourceReceipt,
            Some(pause),
        );
    });

    let stale_result = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let stale_tx = transaction.clone();
        let stale = scope.spawn(move || {
            let failure = runtime_ref
                .commit_retained_ingress_batch(remote_batch(PeerIndex::from(184), [stale_tx]))
                .expect_err("the stale owner Plan returns typed shared contention");
            let (reason, batch) = failure.into_parts();
            (reason, batch.len())
        });
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the stale planner observed owner absence before its resource receipt");
        runtime.with_authority_read_for_foundation(|authority| {
            authority.entries_for_reference().set_shared_ingress_probe(
                SharedIngressProbePhase::BeforeInsertionResourceReceipt,
                None,
            );
        });
        let winner =
            commit_retained_small(&runtime, remote_batch(PeerIndex::from(183), [transaction]));
        assert!(matches!(winner, Ok((1, true))));
        release
            .send(())
            .expect("release the now-stale insertion resource planner");
        stale.join().expect("the stale runtime request joins")
    });
    assert!(matches!(
        stale_result,
        (RetainedIngressBatchFailureReason::SharedContention, 1)
    ));
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.entry(&hash).is_some());
        assert_eq!(authority.owner_count(), 1);
        assert!(authority.primary_projection_consistent());
    });
    assert!(
        runtime
            .effect_observation_for_foundation()
            .queued
            .is_empty()
    );
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
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, Some(pause));
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
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, None);
        assert!(matches!(
            authority.entry(&hash),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.source, PreAcceptedSource::Remote(_))
        ));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_retained_head_classification_flip_is_contention_not_a_route_gap_fault() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the head-classification fixture runtime is valid");
    let transaction = ingress_tx(186);
    let hash = RawTxHash(transaction.hash());
    let peer = PeerIndex::from(186);

    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::AfterRetainedIngressHeadClassification,
            Some(pause),
        );
    });
    let outcome = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let stale_transaction = transaction.clone();
        let commit = scope.spawn(move || {
            let failure = runtime_ref
                .commit_retained_ingress_batch(remote_batch(peer, [stale_transaction]))
                .expect_err("a changed head class must return typed contention");
            let (reason, batch) = failure.into_parts();
            (reason, batch.len())
        });
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the existing-owner effect route is classified once");
        runtime.with_authority_read_for_foundation(|authority| {
            authority.entries_for_reference().set_shared_ingress_probe(
                SharedIngressProbePhase::AfterRetainedIngressHeadClassification,
                None,
            );
        });
        assert!(matches!(
            commit_retained_small(&runtime, remote_batch(peer, [transaction])),
            Ok((1, true))
        ));
        release
            .send(())
            .expect("release the stale classified request");
        commit.join().expect("the classified request thread joins")
    });
    assert!(matches!(
        outcome,
        (RetainedIngressBatchFailureReason::SharedContention, 1)
    ));
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.entry(&hash).is_some());
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_parent_removal_planned_before_shared_child_activation_revalidates_at_apply() {
    const PHASE_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the ingress/removal race fixture runtime is valid");
    let parent_input = OutPoint::new(Byte32::new([201; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(201u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction_with_payload(
            authority,
            parent_tx.clone(),
            201,
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
    let child_tx = ingress_tx_with_cell_dep(202, &parent_output);
    let child = RawTxHash(child_tx.hash());
    let peer = PeerIndex::from(202);

    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_plan_pause(Some(pause));
    });
    let removed = std::thread::scope(|scope| {
        let parent_hash = parent.0.clone();
        let runtime_ref = &runtime;
        let remove = scope.spawn(move || runtime_ref.remove_local_transaction(&parent_hash));
        entered
            .recv_timeout(PHASE_TIMEOUT)
            .expect("the parent removal is paused after its closed Plan");

        let Ok((consumed, remaining, post_commit_fault)) =
            runtime.commit_retained_ingress_batch(remote_batch(peer, [child_tx]))
        else {
            panic!("the shared child commits while the old parent Plan is paused");
        };
        assert_eq!(consumed, 1);
        assert!(remaining.is_empty());
        assert_eq!(post_commit_fault, None);
        release
            .send(())
            .expect("release the stale parent removal Apply");
        remove
            .join()
            .expect("the parent removal thread joins")
            .expect("the shared removal revalidates administrative semantics")
    });
    assert!(removed);

    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_plan_pause(None);
        assert!(authority.entry(&parent).is_none());
        assert!(authority.entry(&child).is_some());
        assert_eq!(
            authority
                .dependency_consumers_for_foundation(&DependencyKey::Cell(parent_output.clone())),
            Some(std::collections::BTreeSet::from([child.clone()]))
        );
        let maintenance = authority
            .dependency_maintenance_observation_for_foundation()
            .expect("the parent loss leaves one bounded dependency successor");
        assert!(matches!(
            maintenance,
            Some((DependencyKey::Cell(key), Some(owner)))
                if key == parent_output && owner == child
        ));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_parent_removal_bound_before_stable_child_growth_is_stale_at_apply() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (parent, parent_output, child_tx) = accepted_parent_and_child(&mut authority, 203, 204);
    let child_hash = RawTxHash(child_tx.hash());
    let removal = authority
        .compile_shared_local_removal(&parent)
        .expect("the parent removal compiles")
        .expect("the parent remains present")
        .bind(&authority)
        .expect("the parent removal binds before child growth");
    assert_ne!(
        removal.dependency_final_support_masks_for_foundation().0,
        0,
        "an Event no-consumer receipt must retain its exact final relation read"
    );
    let (absence_count, fresh) = removal.dependency_fanout_absence_for_foundation();
    assert!(absence_count > 0 && fresh);

    drop(apply_shared_owner_prefix(
        &authority,
        &remote_batch(PeerIndex::from(204), [child_tx]),
    ));
    assert!(!removal.dependency_fanout_absence_for_foundation().1);
    let failure = match removal.apply() {
        Ok(_) => panic!("a stable child must invalidate the bound no-consumer receipt"),
        Err(failure) => failure,
    };
    let (error, effect_wake) = failure.into_parts();
    assert!(matches!(error, ConcurrentRetainedIngressError::Stale));
    assert!(effect_wake.is_none_or(|wake| wake.capacity_released()));
    assert!(authority.entry(&parent).is_some());
    assert!(authority.entry(&child_hash).is_some());
    assert_eq!(
        authority.dependency_consumers_for_foundation(&DependencyKey::Cell(parent_output)),
        Some(std::collections::BTreeSet::from([child_hash]))
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_parent_removal_bound_before_hidden_child_stage_is_stale_at_apply() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (parent, parent_output, child_tx) = accepted_parent_and_child(&mut authority, 205, 206);
    let child_hash = RawTxHash(child_tx.hash());
    let removal = authority
        .compile_shared_local_removal(&parent)
        .expect("the parent removal compiles")
        .expect("the parent remains present")
        .bind(&authority)
        .expect("the parent removal binds before child growth");
    assert_ne!(
        removal.dependency_final_support_masks_for_foundation().0,
        0,
        "a hidden-child Event race must retain its exact final relation read"
    );
    let (absence_count, fresh) = removal.dependency_fanout_absence_for_foundation();
    assert!(absence_count > 0 && fresh);
    let prepared_child = authority
        .compile_shared_retained_ingress_batch(&remote_batch(PeerIndex::from(206), [child_tx]))
        .expect("the child compiles against the hidden loss stage")
        .expect("the child uses the production shared shape")
        .bind(&authority)
        .expect("the child binds to the current generation");
    let (pause, entered, release) = ConcurrentRemovalProbe::new();
    authority
        .entries_for_reference()
        .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, Some(pause));
    let (entered_result, details, removal_result, child_result) = std::thread::scope(|scope| {
        let child = scope.spawn(move || prepared_child.apply());
        let entered_result = entered.recv_timeout(Duration::from_secs(5));
        let details = removal.dependency_fanout_absence_details_for_foundation();
        let removal_result = removal.apply();
        let _released = release.send(());
        let child_result = child.join().expect("the hidden child thread joins");
        (entered_result, details, removal_result, child_result)
    });
    authority
        .entries_for_reference()
        .set_shared_ingress_probe(SharedIngressProbePhase::BothStagesBeforeOwner, None);
    assert!(entered_result.is_ok(), "the child reaches its hidden stage");
    assert!(
        details
            .iter()
            .any(|(_, no_consumers, _, has_consumers, _)| *no_consumers && *has_consumers),
        "the hidden child must violate one no-consumer receipt: {details:?}"
    );
    let failure = match removal_result {
        Ok(_) => panic!("a hidden child must invalidate the bound no-consumer receipt"),
        Err(failure) => failure,
    };
    let (error, effect_wake) = failure.into_parts();
    assert!(matches!(error, ConcurrentRetainedIngressError::Stale));
    assert!(effect_wake.is_none_or(|wake| wake.capacity_released()));
    drop(child_result.expect("the child commits after the stale removal rolls back"));
    assert!(authority.entry(&parent).is_some());
    assert!(authority.entry(&child_hash).is_some());
    assert_eq!(
        authority.dependency_consumers_for_foundation(&DependencyKey::Cell(parent_output)),
        Some(std::collections::BTreeSet::from([child_hash]))
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_disjoint_shared_ingress_plans_activate_safely_in_reverse_compile_order() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let shared_dependency = OutPoint::new(Byte32::new([223; 32]), 0);
    let mut selected = None;
    'left: for left_marker in 120u8..150 {
        let left_tx = ingress_tx_with_cell_dep(left_marker, &shared_dependency);
        let left_hash = RawTxHash(left_tx.hash());
        let Some(left) = authority
            .compile_shared_retained_ingress_batch(&remote_batch(
                PeerIndex::from(usize::from(left_marker)),
                [left_tx],
            ))
            .expect("the first reverse-order plan compiles")
        else {
            continue;
        };
        let left_support = left.physical_write_support_for_foundation(&authority);
        for right_marker in (left_marker + 1)..180 {
            let right_tx = ingress_tx_with_cell_dep(right_marker, &shared_dependency);
            let right_hash = RawTxHash(right_tx.hash());
            let Some(right) = authority
                .compile_shared_retained_ingress_batch(&remote_batch(
                    PeerIndex::from(usize::from(right_marker)),
                    [right_tx],
                ))
                .expect("the second reverse-order plan compiles")
            else {
                continue;
            };
            if left_support.is_disjoint(right.physical_write_support_for_foundation(&authority)) {
                selected = Some((left_hash, left, right_hash, right));
                break 'left;
            }
        }
    }
    let (left_hash, left, right_hash, right) =
        selected.expect("the fixed layout contains a reverse-order disjoint pair");
    drop(
        right
            .bind(&authority)
            .expect("the right plan binds first")
            .apply()
            .expect("the later-compiled plan activates first"),
    );
    let key = DependencyKey::Cell(shared_dependency);
    assert_eq!(
        authority.dependency_consumers_for_foundation(&key),
        Some(std::collections::BTreeSet::from([right_hash.clone()]))
    );
    drop(
        left.bind(&authority)
            .expect("the left plan remains bound")
            .apply()
            .expect("the earlier-compiled plan activates second"),
    );
    assert_eq!(
        authority.dependency_consumers_for_foundation(&key),
        Some(std::collections::BTreeSet::from([left_hash, right_hash]))
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_disjoint_existing_owner_promotions_overlap_inside_their_owner_cuts() {
    const CUT_ENTRY_TIMEOUT: Duration = Duration::from_secs(5);
    let mut selected = None;
    'left: for left_marker in 120u8..150 {
        for right_marker in (left_marker + 1)..180 {
            let authority = TxPoolAuthority::for_foundation(limits());
            let left_tx = ingress_tx(left_marker);
            let right_tx = ingress_tx(right_marker);
            let left_peer = PeerIndex::from(usize::from(left_marker));
            let right_peer = PeerIndex::from(usize::from(right_marker));
            let left_remote = authority
                .compile_shared_retained_ingress_batch(&remote_batch(left_peer, [left_tx.clone()]))
                .expect("the first Remote owner compiles")
                .expect("the first Remote owner has the shared shape");
            let right_remote = authority
                .compile_shared_retained_ingress_batch(&remote_batch(
                    right_peer,
                    [right_tx.clone()],
                ))
                .expect("the second Remote owner compiles")
                .expect("the second Remote owner has the shared shape");
            drop(
                left_remote
                    .bind(&authority)
                    .expect("the first Remote generation remains current")
                    .apply()
                    .expect("the first Remote owner commits"),
            );
            drop(
                right_remote
                    .bind(&authority)
                    .expect("the second Remote generation remains current")
                    .apply()
                    .expect("the second Remote owner commits"),
            );
            let left = authority
                .compile_shared_retained_ingress_batch(&proposal_batch([left_tx]))
                .expect("the first Proposal promotion compiles")
                .expect("the first Proposal promotion has the shared shape");
            let right = authority
                .compile_shared_retained_ingress_batch(&proposal_batch([right_tx]))
                .expect("the second Proposal promotion compiles")
                .expect("the second Proposal promotion has the shared shape");
            if left
                .physical_write_support_for_foundation(&authority)
                .is_disjoint(right.physical_write_support_for_foundation(&authority))
            {
                selected = Some((authority, left, right));
                break 'left;
            }
        }
    }
    let (authority, left, right) =
        selected.expect("the fixed layout contains two disjoint Proposal promotion cuts");
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    authority.entries_for_reference().set_shared_ingress_probe(
        SharedIngressProbePhase::FinalCutBeforeActivation,
        Some(probe),
    );
    std::thread::scope(|scope| {
        let authority_ref = &authority;
        let left = scope.spawn(move || {
            left.bind(authority_ref)
                .expect("the first promotion generation remains current")
                .apply()
        });
        entered
            .recv_timeout(CUT_ENTRY_TIMEOUT)
            .expect("the first promotion enters its owner cut");
        let authority_ref = &authority;
        let right = scope.spawn(move || {
            right
                .bind(authority_ref)
                .expect("the second promotion generation remains current")
                .apply()
        });
        entered
            .recv_timeout(CUT_ENTRY_TIMEOUT)
            .expect("the second disjoint promotion overlaps the first owner cut");
        release.send(()).expect("release the first promotion cut");
        release.send(()).expect("release the second promotion cut");
        drop(
            left.join()
                .expect("the first promotion thread joins")
                .expect("the first promotion commits"),
        );
        drop(
            right
                .join()
                .expect("the second promotion thread joins")
                .expect("the second promotion commits"),
        );
    });
    authority
        .entries_for_reference()
        .set_shared_ingress_probe(SharedIngressProbePhase::FinalCutBeforeActivation, None);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_retained_ingress_batch_refines_the_canonical_proposal_fold() {
    let transactions = vec![ingress_tx(1), ingress_tx(2), ingress_tx(3)];
    let aggregate = runtime_with_authority(TxPoolAuthority::for_foundation(limits()));
    let batch_sequence =
        aggregate.with_authority_read_for_foundation(|authority| authority.clocks().next_sequence);
    assert_eq!(
        commit_all_retained(&aggregate, proposal_batch(transactions.clone())),
        transactions.len()
    );

    let reference = runtime_with_authority(TxPoolAuthority::for_foundation(limits()));
    for transaction in transactions.clone() {
        assert_eq!(
            commit_all_retained(&reference, proposal_batch([transaction])),
            1
        );
    }
    let canonical_next_sequence = ApplySequence(
        batch_sequence.0
            + u128::try_from(transactions.len()).expect("fixture length fits the sequence"),
    );
    let aggregate_snapshot = aggregate.with_authority_read_for_foundation(|authority| {
        assert_eq!(
            authority.clocks().next_sequence,
            ApplySequence(batch_sequence.0 + 1)
        );
        assert!(authority.primary_projection_consistent());
        authority.normalized_snapshot()
    });
    let reference_snapshot = reference.with_authority_read_for_foundation(|authority| {
        assert_eq!(authority.clocks().next_sequence, canonical_next_sequence);
        authority.normalized_snapshot()
    });
    assert!(aggregate_snapshot.equivalent_modulo_atomic_batch_stamp(
        &reference_snapshot,
        batch_sequence,
        canonical_next_sequence,
    ));
}

#[test]
fn uak_retained_ingress_batch_observes_prior_items_in_canonical_order() {
    let transaction = ingress_tx(4);
    let peer = PeerIndex::from(41);
    let aggregate = runtime_with_authority(TxPoolAuthority::for_foundation(limits()));
    assert_eq!(
        commit_all_retained(
            &aggregate,
            remote_batch(peer, [transaction.clone(), transaction.clone()],),
        ),
        2
    );

    let reference = runtime_with_authority(TxPoolAuthority::for_foundation(limits()));
    assert_eq!(
        commit_all_retained(&reference, remote_batch(peer, [transaction.clone()]),),
        1
    );
    assert_eq!(
        commit_all_retained(&reference, remote_batch(peer, [transaction])),
        1
    );
    let aggregate_snapshot = aggregate.with_authority_read_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
        authority.normalized_snapshot()
    });
    let reference_snapshot = reference.with_authority_read_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
        authority.normalized_snapshot()
    });
    assert_eq!(aggregate_snapshot, reference_snapshot);
}

#[test]
fn uak_retained_ingress_batch_applies_resource_pressure_sequentially() {
    let peer = PeerIndex::from(42);
    let transactions = [ingress_tx(5), ingress_tx(6), ingress_tx(7)];
    let runtime = runtime_with_authority(TxPoolAuthority::for_foundation(limits()));
    assert_eq!(
        commit_all_retained(&runtime, remote_batch(peer, transactions)),
        3
    );
    runtime.with_authority_read_for_foundation(|authority| {
        assert_eq!(authority.owner_count(), 2);
        assert_eq!(authority.preaccepted_for_peer_for_reference(peer).len(), 2);
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_retained_ingress_pressure_discards_every_uncommitted_owner_identity() {
    let peer = PeerIndex::from(142);
    let runtime = runtime_with_authority(TxPoolAuthority::for_foundation(limits()));
    let before = runtime.with_authority_read_for_foundation(TxPoolAuthority::clocks);
    assert_eq!(
        commit_all_retained(
            &runtime,
            remote_batch(peer, [ingress_tx(105), ingress_tx(106), ingress_tx(107)],),
        ),
        3
    );
    runtime.with_authority_read_for_foundation(|authority| {
        let after = authority.clocks();
        assert_eq!(authority.owner_count(), 2);
        assert_eq!(
            after.next_version,
            EntryVersion(before.next_version.0 + 2),
            "the rejected third item owns no version"
        );
        assert_eq!(
            after.next_arrival,
            Arrival(before.next_arrival.0 + 2),
            "the rejected third item owns no arrival"
        );
        assert_eq!(
            after.next_sequence,
            ApplySequence(before.next_sequence.0 + 2),
            "the owner prefix and pressure effect are two production Applies"
        );
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
    let RetainedAdmissionDisposition::Retained(resident_plan) = authority
        .plan_retained_admission(
            remote_at_for_foundation(resident, 0, peer, 100, &consensus)
                .expect("resident fixture validates"),
        )
        .expect("resident fixture plans")
    else {
        panic!("resident fixture must retain one owner");
    };
    drop(resident_plan.apply());

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
fn uak_dropped_shared_peer_revocation_restores_every_hidden_projection() {
    let peer = PeerIndex::from(143);
    let authority = TxPoolAuthority::for_foundation(limits());
    let resident = ingress_tx(108);
    let resident_hash = RawTxHash(resident.hash());
    let resident_batch = remote_batch(peer, [resident]);
    let committed = apply_shared_owner_prefix(&authority, &resident_batch);
    assert_eq!(committed.consumed(), 1);
    drop(committed);
    let before = authority.normalized_snapshot();

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
    drop(prepared);

    let after = authority.normalized_snapshot();
    assert!(after.equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1));
    assert!(!authority.peer_fence_hidden_for_reference(peer));
    assert!(!authority.peer_is_banned_for_reference(peer));
    assert!(authority.entry(&resident_hash).is_some());
    assert_eq!(
        authority.preaccepted_for_peer_for_reference(peer),
        vec![resident_hash]
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_stale_shared_peer_revocation_returns_exact_effect_capacity_wake() {
    let peer = PeerIndex::from(144);
    let authority = TxPoolAuthority::for_foundation(limits());
    let resident = ingress_tx(208);
    let resident_hash = RawTxHash(resident.hash());
    drop(apply_shared_owner_prefix(
        &authority,
        &remote_batch(peer, [resident]),
    ));
    let before_effect = authority.effect_observation_for_foundation();
    let before_rows = authority.peer_ingress_row_count_for_reference();

    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(144))
        .build();
    let prepared = authority
        .plan_shared_peer_revocation(&remote_batch(peer, [malformed]))
        .expect("the exact malformed cohort stages")
        .expect("the production shared peer-revocation route is selected");
    assert!(authority.peer_fence_hidden_for_reference(peer));
    assert_ne!(authority.effect_observation_for_foundation(), before_effect);

    authority
        .apply_dependency_loss_during_shared_plan_for_foundation(vec![DependencyKey::Cell(
            OutPoint::new(Byte32::new([208; 32]), 0),
        )])
        .expect("the real dependency frontier changes after the peer plan stages");
    let failure = match prepared.apply() {
        Ok(_) => panic!("the final owner cut must reject the stale cohort removal"),
        Err(failure) => failure,
    };
    let (error, effect_wake) = failure.into_parts();
    assert!(matches!(error, ConcurrentRetainedIngressError::Stale));
    assert!(
        effect_wake.is_some_and(|wake| wake.capacity_released()),
        "explicit rollback returns the exact effect-capacity level"
    );
    assert_eq!(authority.effect_observation_for_foundation(), before_effect);
    assert!(!authority.peer_fence_hidden_for_reference(peer));
    assert!(!authority.peer_is_banned_for_reference(peer));
    assert_eq!(
        authority.peer_ingress_row_count_for_reference(),
        before_rows
    );
    assert!(authority.entry(&resident_hash).is_some());
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
    let second_retirement = authority
        .plan_clear_pool(Byte32::new([147; 32]))
        .expect("a second empty carrier replacement remains allocation-closed")
        .apply();
    drop(second_retirement);
    assert!(
        authority
            .entries_for_reference()
            .same_layout_for_test(&live_layout)
    );
    assert!(authority.peer_is_banned_for_reference(peer));
    assert_eq!(
        authority
            .entries_for_reference()
            .generation_payload_swaps_for_test(),
        swaps_before + 2 * crate::authority::shard::AUTHORITY_SHARD_COUNT
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
    let live_layout = authority.entries_for_reference().clone();
    drop(
        authority
            .plan_clear_pool(Byte32::new([148; 32]))
            .expect("the expired physical fence survives one empty carrier replacement")
            .apply(),
    );
    assert!(
        authority
            .entries_for_reference()
            .same_layout_for_test(&live_layout)
    );
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
}

#[test]
fn uak_retained_ingress_batch_commits_only_the_longest_effect_prefix() {
    let effect_limits =
        EffectLimits::for_foundation().with_remote_effects_per_batch_for_foundation(2);
    let authority = TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("fixture effect limits are valid");
    let peer = PeerIndex::from(44);
    let invalid = |marker| {
        ingress_tx(marker)
            .as_advanced_builder()
            .version(1u32)
            .build()
    };
    let batch = remote_batch(peer, [invalid(11), invalid(12), invalid(13)]);
    let prepared = authority
        .plan_shared_retained_effect_prefix(&batch)
        .expect("a complete effect prefix fits")
        .expect("the batch begins with an effect-only production prefix");
    let committed = prepared.apply();
    assert_eq!(committed.consumed(), 2);
    drop(committed);
    assert_eq!(authority.owner_count(), 0);
    assert!(authority.primary_projection_consistent());
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
fn uak_shared_retained_pressure_effect_uses_no_owner_apply() {
    let peer = PeerIndex::from(145);
    let authority = TxPoolAuthority::for_foundation(limits());
    let fixture = remote_batch(peer, [ingress_tx(114), ingress_tx(115)]);
    let committed = apply_shared_owner_prefix(&authority, &fixture);
    assert_eq!(committed.consumed(), 2);
    drop(committed);
    assert_eq!(authority.owner_count(), 2);

    let prepared = authority
        .plan_shared_retained_effect_prefix(&remote_batch(peer, [ingress_tx(116)]))
        .expect("the pressure outcome plans")
        .expect("pressure produces an effect-only shared prefix");
    let committed = prepared.apply();
    assert_eq!(committed.consumed(), 1);
    drop(committed);
    assert_eq!(authority.owner_count(), 2);
    assert_eq!(
        authority.effect_observation_for_foundation().queued.len(),
        1
    );
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

#[test]
fn uak_retained_ingress_batch_keeps_recovery_payload_variant_unchanged() {
    let raw = ingress_tx(21);
    let recovery = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"recovery").pack()])
        .build();
    let proposal_variant = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
        .build();
    let mut authority = TxPoolAuthority::for_foundation(limits());
    drop(
        authority
            .plan_admission(
                ValidatedAdmission::recovery(recovery, PoolGeneration(0))
                    .expect("recovery witness variant is valid"),
            )
            .expect("recovery witness enters retained ownership")
            .apply(),
    );
    let before = authority.normalized_snapshot();

    let batch = proposal_batch([proposal_variant]);
    let committed = authority
        .plan_shared_retained_effect_prefix(&batch)
        .expect("the payload variant is an ordinary batch outcome")
        .expect("the closed payload variant is a no-owner production prefix")
        .apply();
    assert_eq!(committed.consumed(), 1);
    drop(committed);
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_retained_ingress_batch_plan_failure_and_drop_are_mutation_free() {
    let transactions = vec![ingress_tx(14), ingress_tx(15)];
    let dropped = TxPoolAuthority::for_foundation(limits());
    let before_drop = dropped.normalized_snapshot();
    let prepared = dropped
        .compile_shared_retained_ingress_batch(&proposal_batch(transactions.clone()))
        .expect("the batch plans without mutation")
        .expect("the all-owner batch has a shared production shape");
    drop(prepared);
    assert!(
        dropped
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before_drop, 2, 2, 1),
        "the dropped retained batch burns exactly its two owner identities and one Apply stamp"
    );

    let mut exhausted = TxPoolAuthority::for_foundation(limits());
    exhausted.force_next_version(EntryVersion(u128::MAX));
    let before_error = exhausted.normalized_snapshot();
    assert!(matches!(
        exhausted.compile_shared_retained_ingress_batch(&proposal_batch(transactions)),
        Err(PlanError::Fault(AuthorityFault::CounterExhausted))
    ));
    assert_eq!(exhausted.normalized_snapshot(), before_error);
}

#[test]
fn uak_retained_ingress_batch_noop_has_no_apply_or_clock_advance() {
    let batch = RetainedAdmissionBatch::new(
        RetainedIngressAttempt::ProposalUnavailable,
        VecDeque::from([RetainedIngressAttempt::ProposalUnavailable]),
    )
    .expect("the unavailable proposal batch is homogeneous");
    let authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    let committed = authority
        .plan_shared_retained_effect_prefix(&batch)
        .expect("a no-owner proposal outcome is ordinary")
        .expect("the complete batch is a no-owner production prefix")
        .apply();

    assert_eq!(committed.consumed(), 2);
    drop(committed);
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_retained_ingress_batch_pressure_noop_does_not_require_an_apply_sequence() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let resident = (30..38).map(ingress_tx).collect::<Vec<_>>();
    let resident = proposal_batch(resident);
    let committed = apply_shared_owner_prefix(&authority, &resident);
    assert_eq!(committed.consumed(), 8);
    drop(committed);
    assert_eq!(authority.owner_count(), 8);

    authority.force_next_sequence(ApplySequence(u128::MAX));
    let before = authority.normalized_snapshot();
    let pressure = proposal_batch([ingress_tx(38)]);
    let committed = authority
        .plan_shared_retained_effect_prefix(&pressure)
        .expect("a proposal excluded by projected pressure performs no Apply")
        .expect("the pressure item is a no-owner production prefix")
        .apply();

    assert_eq!(committed.consumed(), 1);
    drop(committed);
    assert_eq!(authority.normalized_snapshot(), before);
}
