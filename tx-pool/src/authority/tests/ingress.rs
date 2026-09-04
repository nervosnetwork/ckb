use super::super::{
    effect::CommittedEffect,
    ingress::{
        BoundedTransaction, DirectCommand, DirectIngressTransaction, RemoteIngressPressure,
        RetainedIngressAttempt, RetainedIngressBackpressure, RetainedIngressBoundaryError, direct,
        proposal,
        test_support::{IngressRejectionCommit, remote_at_for_foundation},
    },
    plan::{
        AuthorityFault, Backpressure, CommittedRetainedAdmissionBatch, PlanError, TxPoolAuthority,
    },
    runtime::AuthorityRuntime,
    state::{PoolGeneration, PreAcceptedSource, ProposalBase, RemoteDeadline, ValidatedAdmission},
};
use super::foundation::{
    accept_remote_transaction_with_payload, genesis_snapshot, limits, resolved_payload_with_facts,
    runtime_config,
};
use ckb_chain_spec::consensus::{ConsensusBuilder, MAX_BLOCK_INTERVAL};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::Pack,
};

const REMOTE_RESIDENCY_BLOCKS: u64 = 100;

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

fn bounded(transaction: TransactionView) -> BoundedTransaction {
    BoundedTransaction::try_new(transaction).expect("ingress fixture transaction is bounded")
}

fn direct_input(transaction: TransactionView) -> DirectIngressTransaction {
    bounded(transaction).into_direct()
}

#[test]
fn uak_remote_ingress_derives_source_and_historical_residency() {
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(17);
    let declared_cycles = 41;
    let admitted_at_secs = 900;
    let admission = remote_at_for_foundation(
        ingress_tx(1),
        declared_cycles,
        peer,
        admitted_at_secs,
        &consensus,
    )
    .expect("foundation transaction passes non-contextual validation");

    let admission = admission.admission_for_foundation();
    let PreAcceptedSource::Remote(remote) = admission.source else {
        panic!("remote ingress must construct a remote source");
    };
    assert_eq!(remote.residency.peer, peer);
    assert_eq!(
        remote.residency.expires_at,
        RemoteDeadline(
            admitted_at_secs.saturating_add(REMOTE_RESIDENCY_BLOCKS * MAX_BLOCK_INTERVAL)
        )
    );
    assert_eq!(
        remote.payload_policy.declared_cycles(),
        Some(declared_cycles)
    );
}

#[test]
fn uak_remote_ingress_rejects_a_declaration_above_consensus_max_before_ownership() {
    let consensus = ConsensusBuilder::default().build();
    let declared_cycles = consensus
        .max_block_cycles()
        .checked_add(1)
        .expect("the default consensus maximum leaves one hostile declaration");
    let attempt = remote_at_for_foundation(
        ingress_tx(8),
        declared_cycles,
        PeerIndex::from(18),
        901,
        &consensus,
    )
    .expect_err("d > M must terminate at ingress without constructing an owner");
    assert!(declared_cycles > consensus.max_block_cycles());
    assert!(attempt.is_malformed_remote());
}

#[test]
fn uak_proposal_ingress_is_trusted_without_a_synthetic_context_token() {
    let consensus = ConsensusBuilder::default().build();
    let admission = proposal(bounded(ingress_tx(2)), &consensus)
        .into_validated_for_foundation()
        .expect("foundation transaction passes non-contextual validation");

    let admission = admission.admission_for_foundation();
    assert_eq!(
        admission.source,
        PreAcceptedSource::Proposal {
            base: ProposalBase::Trusted
        }
    );
    assert_eq!(admission.source.ingress_peer(), None);
}

#[test]
fn uak_retained_ingress_rejects_malformed_transactions_before_retention() {
    let consensus = ConsensusBuilder::default().build();
    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();

    let error = remote_at_for_foundation(cellbase, 0, PeerIndex::from(19), 0, &consensus)
        .expect_err("a loose cellbase transaction is malformed");
    assert!(matches!(
        error,
        RetainedIngressAttempt::Rejected(ref rejected)
            if rejected.reason_for_foundation().is_malformed()
    ));
}

#[test]
fn uak_direct_ingress_seals_non_contextual_validation_without_retention() {
    let consensus = ConsensusBuilder::default().build();
    let transaction = ingress_tx(9);
    let expected = transaction.witness_hash();
    let transaction = direct_input(transaction);
    let validated = direct(&transaction, &consensus, DirectCommand::TestAccept)
        .expect("a valid direct transaction acquires the computation capability");
    let (validated, command) = validated.into_parts();
    assert_eq!(command, DirectCommand::TestAccept);
    assert_eq!(validated.witness_hash(), expected);

    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    let cellbase_witness_hash = cellbase.witness_hash();
    let cellbase = direct_input(cellbase);
    let rejection = direct(&cellbase, &consensus, DirectCommand::Local)
        .expect_err("a malformed direct transaction cannot enter resolution");
    assert_eq!(rejection.command(), DirectCommand::Local);
    assert_eq!(
        rejection.transaction().witness_hash(),
        cellbase_witness_hash
    );
    assert!(rejection.reason().is_malformed());
}

#[test]
fn uak_malformed_remote_precheck_commits_the_peer_fence_with_its_rejection() {
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(24);
    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    let expected_hash = super::super::state::RawTxHash(cellbase.hash());
    let RetainedIngressAttempt::Rejected(rejection) =
        remote_at_for_foundation(cellbase, 0, peer, 0, &consensus)
            .expect_err("a loose cellbase transaction is malformed")
    else {
        panic!("non-contextual rejection must retain its exact public evidence");
    };
    let authority = TxPoolAuthority::for_foundation(limits());
    let committed = authority
        .commit_retained_rejection_for_foundation(rejection)
        .expect("malformed Remote precheck plans one peer revocation");
    drop(committed);
    assert!(authority.peer_is_banned_for_reference(peer));

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("peer revocation effect is committed");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::PeerCohortRevoked(revocation)]
            if revocation.peer() == peer
                && revocation.culprit().is_some_and(|culprit| culprit.tx_hash() == &expected_hash)
    ));
}

#[test]
fn uak_nonmalformed_remote_precheck_rejects_without_banning_the_peer() {
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(25);
    let wrong_version = ingress_tx(6).as_advanced_builder().version(1u32).build();
    let expected_hash = super::super::state::RawTxHash(wrong_version.hash());
    let RetainedIngressAttempt::Rejected(rejection) =
        remote_at_for_foundation(wrong_version, 0, peer, 0, &consensus)
            .expect_err("the consensus transaction version is rejected")
    else {
        panic!("non-contextual rejection must retain its exact public evidence");
    };
    assert!(!rejection.reason_for_foundation().is_malformed());
    let authority = TxPoolAuthority::for_foundation(limits());
    drop(
        authority
            .commit_retained_rejection_for_foundation(rejection)
            .expect("ordinary Remote rejection plans one effect"),
    );
    assert!(!authority.peer_is_banned_for_reference(peer));

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("validation rejection effect is committed");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Rejected(super::super::effect::CommittedRejection::Validation {
            tx,
            audience,
            ..
        })] if super::super::state::RawTxHash(tx.hash()) == expected_hash
            && audience.ingress_peer() == Some(peer)
    ));
}

#[test]
fn uak_remote_preaccepted_duplicate_releases_filter_without_a_second_owner() {
    let consensus = ConsensusBuilder::default().build();
    let transaction = ingress_tx(3);
    let first = remote_at_for_foundation(
        transaction.clone(),
        10,
        PeerIndex::from(20),
        100,
        &consensus,
    )
    .expect("first Remote ingress is valid");
    let hash = first.admission_for_foundation().identity.raw.clone();
    let authority = TxPoolAuthority::for_foundation(limits());
    drop(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(first))
            .expect("first Remote ingress commits through the production shared route"),
    );
    let before_version = authority
        .entry(&hash)
        .expect("first Remote owner exists")
        .record()
        .version;

    let duplicate = remote_at_for_foundation(transaction, 20, PeerIndex::from(21), 101, &consensus)
        .expect("duplicate Remote ingress is structurally valid");
    drop(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(duplicate))
            .expect("duplicate Remote ingress commits one filter release"),
    );
    let owner = authority.entry(&hash).expect("duplicate keeps first owner");
    assert_eq!(owner.record().version, before_version);
    assert_eq!(owner.ingress_peer(), Some(PeerIndex::from(20)));

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("filter release is committed");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::RemoteIngressReleased(release)] if release.tx_hash() == &hash
    ));
}

#[test]
fn uak_delayed_revoked_remote_ingress_commits_a_later_filter_release() {
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(23);
    let transaction = ingress_tx(5);
    let expected_hash = super::super::state::RawTxHash(transaction.hash());
    let mut authority = TxPoolAuthority::for_foundation(limits());

    drop(
        authority
            .plan_peer_revocation_for_foundation(peer)
            .expect("the peer fence plans independently of a resident owner")
            .apply(),
    );
    let reset = authority
        .effect_publication_receipt_for_foundation()
        .expect("peer revocation commits one reset");
    assert!(matches!(
        reset.effects(),
        [CommittedEffect::PeerCohortRevoked(revocation)] if revocation.peer() == peer
    ));
    drop(
        authority
            .apply_effect_settlement_for_foundation(reset.complete_for_foundation())
            .expect("the one-shot reset may be consumed before delayed ingress"),
    );

    let delayed = remote_at_for_foundation(transaction, 10, peer, 100, &consensus)
        .expect("the already-queued Remote message remains structurally valid");
    drop(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(delayed))
            .expect("the peer fence commits exact relay cleanup for delayed ingress"),
    );
    assert!(authority.entry(&expected_hash).is_none());

    let release = authority
        .effect_publication_receipt_for_foundation()
        .expect("the delayed Remote ingress commits a later release");
    assert!(matches!(
        release.effects(),
        [CommittedEffect::RemoteIngressReleased(release)]
            if release.tx_hash() == &expected_hash
    ));
}

#[test]
fn uak_peer_fence_saturation_revalidates_the_oldest_delayed_session() {
    let consensus = ConsensusBuilder::default().build();
    let first = PeerIndex::from(31);
    let second = PeerIndex::from(32);
    let transaction = ingress_tx(7);
    let expected_hash = super::super::state::RawTxHash(transaction.hash());
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.set_peer_ban_limit_for_foundation(1);

    for peer in [first, second] {
        drop(
            authority
                .plan_peer_revocation_for_foundation(peer)
                .expect("the bounded peer fence plans")
                .apply(),
        );
    }
    assert!(!authority.peer_is_banned_for_reference(first));
    assert!(authority.peer_is_banned_for_reference(second));

    let delayed = remote_at_for_foundation(transaction, 10, first, 100, &consensus)
        .expect("the delayed Remote message remains structurally valid");
    drop(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(delayed))
            .expect("an evicted oldest fence falls back to complete bounded admission"),
    );
    assert!(authority.entry(&expected_hash).is_some());
}

#[test]
fn uak_remote_accepted_duplicate_publishes_only_the_observed_accepted_fact() {
    let consensus = ConsensusBuilder::default().build();
    let transaction = ingress_tx(4);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let chain_inputs = transaction.input_pts_iter().collect();
    let payload = resolved_payload_with_facts(
        &transaction,
        Vec::new(),
        chain_inputs,
        Capacity::shannons(1),
    );
    let hash = accept_remote_transaction_with_payload(
        &mut authority,
        transaction.clone(),
        22,
        super::super::state::AcceptedStatus::Pending,
        payload,
    );

    let duplicate = remote_at_for_foundation(transaction, 0, PeerIndex::from(23), 102, &consensus)
        .expect("Accepted duplicate is structurally valid");
    drop(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(duplicate))
            .expect("Accepted duplicate commits its exact acknowledgement"),
    );

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("Accepted duplicate effect is committed");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Accepted(super::super::effect::CommittedAcceptance::Duplicate {
            tx_hash,
            requesting_peer: Some(peer),
        })] if tx_hash == &hash && *peer == PeerIndex::from(23)
    ));
}

#[test]
fn uak_repeated_proposal_ingress_is_a_mutation_free_observation() {
    let consensus = ConsensusBuilder::default().build();
    let transaction = ingress_tx(5);
    let first = proposal(bounded(transaction.clone()), &consensus)
        .into_validated_for_foundation()
        .expect("first Proposal ingress is valid");
    let authority = TxPoolAuthority::for_foundation(limits());
    drop(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(first))
            .expect("first Proposal ingress commits through the production shared route"),
    );
    let before = authority.normalized_snapshot();

    let repeated = proposal(bounded(transaction), &consensus)
        .into_validated_for_foundation()
        .expect("repeated Proposal ingress is valid");
    assert!(matches!(
        authority
            .commit_retained_attempt_for_foundation(RetainedIngressAttempt::Validated(repeated))
            .expect("repeated Proposal has a deterministic disposition"),
        CommittedRetainedAdmissionBatch::Unchanged { consumed: 1 }
    ));
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_recovery_proposal_payload_variant_is_a_closed_no_change_outcome() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("production authority runtime fixture is valid");
    let raw = ingress_tx(9);
    let recovery = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"recovery").pack()])
        .build();
    let proposal_variant = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
        .build();
    runtime
        .admit(
            ValidatedAdmission::recovery(recovery, PoolGeneration(0))
                .expect("recovery witness variant is valid"),
        )
        .expect("recovery witness enters retained ownership");
    let before = runtime.normalized_snapshot_for_foundation();

    let attempt = proposal(bounded(proposal_variant), snapshot.consensus());
    let batch = super::super::ingress::RetainedAdmissionBatch::new(
        attempt,
        std::collections::VecDeque::new(),
    )
    .expect("one proposal forms a bounded retained-ingress batch");
    let (consumed, remaining, fault) = match runtime.commit_retained_ingress_batch(batch) {
        Ok(committed) => committed,
        Err(_) => panic!("payload variant is an ordinary retained-ingress outcome"),
    };
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(fault, None);
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);
}

#[test]
fn uak_remote_no_owner_pressure_commits_the_exact_filter_release_effect() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("production authority runtime fixture is valid");
    let transaction = ingress_tx(10);
    let expected_hash = super::super::state::RawTxHash(transaction.hash());
    let peer = PeerIndex::from(30);

    assert_eq!(
        runtime
            .reject_remote_ingress_pressure(
                transaction,
                peer,
                RemoteIngressPressure::PeerResources,
            )
            .expect("bounded Remote pressure commits one terminal disposition"),
        IngressRejectionCommit
    );

    runtime.with_authority_for_foundation(|authority| {
        assert!(
            authority.entry(&expected_hash).is_none(),
            "terminal pressure must not manufacture a lifecycle owner"
        );
        let lease = authority
            .effect_publication_receipt_for_foundation()
            .expect("the no-owner disposition committed one effect");
        assert!(matches!(
            lease.effects(),
            [CommittedEffect::Rejected(super::super::effect::CommittedRejection::Validation {
                tx,
                audience,
                reason,
            })] if super::super::state::RawTxHash(tx.hash()) == expected_hash
                && audience.ingress_peer() == Some(peer)
                && matches!(reason.reject(), crate::error::Reject::Full(_))
        ));
    });
}

#[test]
fn uak_retained_ingress_boundary_keeps_legal_pressure_out_of_fail_stop() {
    assert_eq!(
        RetainedIngressBoundaryError::from_plan(PlanError::Backpressure(
            Backpressure::PeerResources,
        )),
        RetainedIngressBoundaryError::Backpressure(RetainedIngressBackpressure::PeerResources,)
    );
    assert_eq!(
        RetainedIngressBoundaryError::from_plan(PlanError::Backpressure(
            Backpressure::EffectCapacity,
        )),
        RetainedIngressBoundaryError::Backpressure(RetainedIngressBackpressure::EffectCapacity,)
    );
    assert_eq!(
        RetainedIngressBoundaryError::from_plan(PlanError::Backpressure(
            Backpressure::ProposalCollision,
        )),
        RetainedIngressBoundaryError::Backpressure(RetainedIngressBackpressure::ProposalCollision,)
    );
    assert_eq!(
        RetainedIngressBoundaryError::from_plan(PlanError::Backpressure(
            Backpressure::AcceptedResources,
        )),
        RetainedIngressBoundaryError::Fault(AuthorityFault::ResourceProjection)
    );
}
