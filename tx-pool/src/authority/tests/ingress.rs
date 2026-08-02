use super::super::{
    effect::CommittedEffect,
    ingress::{
        DirectCommand, RetainedIngressCommit, RetainedIngressError, direct, proposal,
        remote_at_for_foundation,
    },
    plan::{RetainedAdmissionDisposition, TxPoolAuthority},
    runtime::AuthorityRuntime,
    state::{PayloadPolicy, PreAcceptedSource, ProposalBase, RemoteDeadline},
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
        remote.payload_policy,
        PayloadPolicy::RemoteDeclaredCycles(declared_cycles)
    );
}

#[test]
fn uak_proposal_ingress_is_trusted_without_a_synthetic_context_token() {
    let consensus = ConsensusBuilder::default().build();
    let admission = proposal(ingress_tx(2), &consensus)
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
        RetainedIngressError::Rejected(ref rejected)
            if rejected.reason_for_foundation().is_malformed()
    ));
}

#[test]
fn uak_direct_ingress_seals_non_contextual_validation_without_retention() {
    let consensus = ConsensusBuilder::default().build();
    let transaction = ingress_tx(9);
    let expected = transaction.witness_hash();
    let validated = direct(&transaction, &consensus, DirectCommand::TestAccept)
        .expect("a valid direct transaction acquires the computation capability");
    let (validated, command) = validated.into_parts();
    assert_eq!(command, DirectCommand::TestAccept);
    assert_eq!(validated.witness_hash(), expected);

    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    let rejection = direct(&cellbase, &consensus, DirectCommand::Local)
        .expect_err("a malformed direct transaction cannot enter resolution");
    assert_eq!(rejection.command(), DirectCommand::Local);
    assert_eq!(
        rejection.transaction().witness_hash(),
        cellbase.witness_hash()
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
    let RetainedIngressError::Rejected(rejection) =
        remote_at_for_foundation(cellbase, 0, peer, 0, &consensus)
            .expect_err("a loose cellbase transaction is malformed")
    else {
        panic!("non-contextual rejection must retain its exact public evidence");
    };
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let committed = authority
        .plan_retained_ingress_rejection(rejection)
        .expect("malformed Remote precheck plans one peer revocation")
        .apply();
    drop(committed);
    assert!(authority.peer_is_banned_for_reference(peer));

    let lease = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("peer revocation effect is committed")
        .apply()
        .into_effect_lease()
        .expect("effect checkout returns its lease");
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
    let RetainedIngressError::Rejected(rejection) =
        remote_at_for_foundation(wrong_version, 0, peer, 0, &consensus)
            .expect_err("the consensus transaction version is rejected")
    else {
        panic!("non-contextual rejection must retain its exact public evidence");
    };
    assert!(!rejection.reason_for_foundation().is_malformed());
    let mut authority = TxPoolAuthority::for_foundation(limits());
    drop(
        authority
            .plan_retained_ingress_rejection(rejection)
            .expect("ordinary Remote rejection plans one effect")
            .apply(),
    );
    assert!(!authority.peer_is_banned_for_reference(peer));

    let lease = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("validation rejection effect is committed")
        .apply()
        .into_effect_lease()
        .expect("effect checkout returns its lease");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Rejected(super::super::effect::CommittedRejection::Validation {
            tx,
            audience,
            ..
        })] if super::super::state::RawTxHash(tx.hash()) == expected_hash
            && audience.ingress_peer == Some(peer)
            && audience.blame_peer == Some(peer)
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
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let RetainedAdmissionDisposition::Retained(first) = authority
        .plan_retained_admission(first)
        .expect("first Remote ingress plans")
    else {
        panic!("first Remote ingress must retain one owner");
    };
    drop(first.apply());
    let before_version = authority
        .entry(&hash)
        .expect("first Remote owner exists")
        .record()
        .version;

    let duplicate = remote_at_for_foundation(transaction, 20, PeerIndex::from(21), 101, &consensus)
        .expect("duplicate Remote ingress is structurally valid");
    let RetainedAdmissionDisposition::RemoteReleased(release) = authority
        .plan_retained_admission(duplicate)
        .expect("duplicate Remote ingress plans one filter release")
    else {
        panic!("a PreAccepted duplicate must not claim Accepted ownership");
    };
    drop(release.apply());
    let owner = authority.entry(&hash).expect("duplicate keeps first owner");
    assert_eq!(owner.record().version, before_version);
    assert_eq!(owner.ingress_peer(), Some(PeerIndex::from(20)));

    let lease = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("filter release is committed")
        .apply()
        .into_effect_lease()
        .expect("effect checkout returns its lease");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::RemoteIngressReleased { tx_hash }] if tx_hash == &hash
    ));
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
    let RetainedAdmissionDisposition::AcceptedDuplicate(duplicate) = authority
        .plan_retained_admission(duplicate)
        .expect("Accepted duplicate plans its exact acknowledgement")
    else {
        panic!("an Accepted duplicate must use the Accepted observation path");
    };
    drop(duplicate.apply());

    let lease = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("Accepted duplicate effect is committed")
        .apply()
        .into_effect_lease()
        .expect("effect checkout returns its lease");
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
    let first = proposal(transaction.clone(), &consensus).expect("first Proposal ingress is valid");
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let RetainedAdmissionDisposition::Retained(first) = authority
        .plan_retained_admission(first)
        .expect("first Proposal ingress plans")
    else {
        panic!("first Proposal ingress must retain one owner");
    };
    drop(first.apply());
    let before = authority.normalized_snapshot();

    let repeated = proposal(transaction, &consensus).expect("repeated Proposal ingress is valid");
    assert!(matches!(
        authority
            .plan_retained_admission(repeated)
            .expect("repeated Proposal has a deterministic disposition"),
        RetainedAdmissionDisposition::ProposalUnchanged
    ));
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_runtime_retained_ingress_adapter_preserves_closed_source_outcomes() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("production authority runtime fixture is valid");
    let transaction = ingress_tx(7);
    assert_eq!(
        runtime
            .submit_remote_ingress(transaction.clone(), 0, PeerIndex::from(26),)
            .expect("first Remote ingress commits"),
        RetainedIngressCommit::Retained
    );
    assert_eq!(
        runtime
            .submit_remote_ingress(transaction.clone(), 0, PeerIndex::from(27),)
            .expect("duplicate Remote ingress commits its release"),
        RetainedIngressCommit::RemoteReleased
    );

    let proposal = ingress_tx(8);
    assert_eq!(
        runtime
            .submit_proposal_ingress(proposal.clone())
            .expect("first Proposal ingress commits"),
        RetainedIngressCommit::Retained
    );
    assert_eq!(
        runtime
            .submit_proposal_ingress(proposal)
            .expect("repeated Proposal ingress is deterministic"),
        RetainedIngressCommit::ProposalUnchanged
    );

    let malformed = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .build();
    assert_eq!(
        runtime
            .submit_remote_ingress(malformed, 0, PeerIndex::from(28))
            .expect("malformed Remote ingress commits its peer disposition"),
        RetainedIngressCommit::Rejected
    );
}
