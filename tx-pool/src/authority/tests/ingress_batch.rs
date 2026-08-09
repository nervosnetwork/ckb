use super::super::{
    effect::EffectLimits,
    ingress::{
        RetainedAdmissionBatch, RetainedIngressAttempt, RetainedIngressError, proposal,
        test_support::remote_at_for_foundation,
    },
    plan::{
        AuthorityFault, PlanError, TxPoolAuthority, test_support::RetainedAdmissionDisposition,
    },
    state::{ApplySequence, EntryVersion, PoolGeneration, RawTxHash, ValidatedAdmission},
};
use super::foundation::{
    accept_remote_transaction_with_payload, limits, resolved_payload_with_facts,
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::Pack,
};
use std::collections::VecDeque;

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

fn proposal_batch(
    transactions: impl IntoIterator<Item = TransactionView>,
) -> RetainedAdmissionBatch {
    let consensus = ConsensusBuilder::default().build();
    let mut attempts = transactions
        .into_iter()
        .map(|transaction| {
            proposal(transaction, &consensus)
                .map(RetainedIngressAttempt::Validated)
                .unwrap_or_else(|error| match error {
                    RetainedIngressError::Rejected(rejection) => {
                        RetainedIngressAttempt::Rejected(rejection)
                    }
                    RetainedIngressError::Admission(error) => {
                        panic!("fixture admission failed unexpectedly: {error:?}")
                    }
                })
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
                .unwrap_or_else(|error| match error {
                    RetainedIngressError::Rejected(rejection) => {
                        RetainedIngressAttempt::Rejected(rejection)
                    }
                    RetainedIngressError::Admission(error) => {
                        panic!("fixture admission failed unexpectedly: {error:?}")
                    }
                })
        })
        .collect::<VecDeque<_>>();
    let head = attempts
        .pop_front()
        .expect("fixture constructs a non-empty remote batch");
    RetainedAdmissionBatch::new(head, attempts).expect("fixture batch is homogeneous")
}

fn apply_sequential(authority: &mut TxPoolAuthority, batch: RetainedAdmissionBatch) {
    for attempt in batch.into_attempts() {
        match attempt {
            RetainedIngressAttempt::Validated(ingress) => {
                match authority
                    .plan_retained_admission(ingress)
                    .expect("canonical retained item plans")
                {
                    RetainedAdmissionDisposition::Retained(plan)
                    | RetainedAdmissionDisposition::AcceptedDuplicate(plan)
                    | RetainedAdmissionDisposition::RemoteReleased(plan) => drop(plan.apply()),
                    RetainedAdmissionDisposition::ProposalUnchanged
                    | RetainedAdmissionDisposition::ProposalPayloadVariant => {}
                }
            }
            RetainedIngressAttempt::Rejected(rejection) => drop(
                authority
                    .plan_retained_ingress_rejection(rejection)
                    .expect("canonical rejection plans")
                    .apply(),
            ),
            RetainedIngressAttempt::ProposalUnavailable => {}
        }
    }
}

#[test]
fn uak_retained_ingress_batch_refines_the_canonical_proposal_fold() {
    let transactions = vec![ingress_tx(1), ingress_tx(2), ingress_tx(3)];
    let mut aggregate = TxPoolAuthority::for_foundation(limits());
    let batch_sequence = aggregate.clocks().next_sequence;
    let committed = aggregate
        .plan_retained_admission_batch(&proposal_batch(transactions.clone()))
        .expect("the aggregate proposal batch plans")
        .apply();
    assert_eq!(committed.consumed(), transactions.len());
    drop(committed);

    let mut reference = TxPoolAuthority::for_foundation(limits());
    apply_sequential(&mut reference, proposal_batch(transactions.clone()));
    let canonical_next_sequence = ApplySequence(
        batch_sequence.0
            + u128::try_from(transactions.len()).expect("fixture length fits the sequence"),
    );

    assert_eq!(
        aggregate.clocks().next_sequence,
        ApplySequence(batch_sequence.0 + 1)
    );
    assert_eq!(reference.clocks().next_sequence, canonical_next_sequence);
    assert!(
        aggregate
            .normalized_snapshot()
            .equivalent_modulo_atomic_batch_stamp(
                &reference.normalized_snapshot(),
                batch_sequence,
                canonical_next_sequence,
            )
    );
    assert!(aggregate.primary_projection_consistent());
}

#[test]
fn uak_retained_ingress_batch_observes_prior_items_in_canonical_order() {
    let transaction = ingress_tx(4);
    let peer = PeerIndex::from(41);
    let mut aggregate = TxPoolAuthority::for_foundation(limits());
    let batch_sequence = aggregate.clocks().next_sequence;
    let committed = aggregate
        .plan_retained_admission_batch(&remote_batch(
            peer,
            [transaction.clone(), transaction.clone()],
        ))
        .expect("the ordered remote batch plans")
        .apply();
    assert_eq!(committed.consumed(), 2);
    drop(committed);

    let mut reference = TxPoolAuthority::for_foundation(limits());
    apply_sequential(
        &mut reference,
        remote_batch(peer, [transaction.clone(), transaction]),
    );
    let canonical_next_sequence = ApplySequence(batch_sequence.0 + 2);
    assert!(
        aggregate
            .normalized_snapshot()
            .equivalent_modulo_atomic_batch_stamp(
                &reference.normalized_snapshot(),
                batch_sequence,
                canonical_next_sequence,
            )
    );
    assert!(aggregate.primary_projection_consistent());
}

#[test]
fn uak_retained_ingress_batch_applies_resource_pressure_sequentially() {
    let peer = PeerIndex::from(42);
    let transactions = [ingress_tx(5), ingress_tx(6), ingress_tx(7)];
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let committed = authority
        .plan_retained_admission_batch(&remote_batch(peer, transactions))
        .expect("bounded pressure is a committed item outcome")
        .apply();
    assert_eq!(committed.consumed(), 3);
    drop(committed);
    assert_eq!(authority.owner_count(), 2);
    assert_eq!(authority.preaccepted_for_peer_for_reference(peer).len(), 2);
    assert!(authority.primary_projection_consistent());
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
    let committed = authority
        .plan_retained_admission_batch(&remote_batch(peer, [fresh, malformed]))
        .expect("malformed cohort plans one peer revocation")
        .apply();
    assert_eq!(committed.consumed(), 2);
    drop(committed);

    assert!(authority.entry(&resident_hash).is_none());
    assert!(authority.entry(&fresh_hash).is_none());
    assert!(authority.entry(&accepted_hash).is_some());
    assert!(authority.peer_is_banned_for_reference(peer));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_retained_ingress_batch_commits_only_the_longest_effect_prefix() {
    let effect_limits =
        EffectLimits::for_foundation().with_remote_effects_per_batch_for_foundation(2);
    let mut authority = TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
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
        .plan_retained_admission_batch(&batch)
        .expect("a complete effect prefix fits");
    assert_eq!(prepared.consumed(), 2);
    let committed = prepared.apply();
    assert_eq!(committed.consumed(), 2);
    drop(committed);
    assert_eq!(authority.owner_count(), 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_retained_ingress_batch_effect_cut_keeps_only_prior_owner_mutations() {
    let effect_limits =
        EffectLimits::for_foundation().with_remote_effects_per_batch_for_foundation(1);
    let mut authority = TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("fixture effect limits are valid");
    let peer = PeerIndex::from(45);
    let transactions = [
        ingress_tx(17),
        ingress_tx(18),
        ingress_tx(19),
        ingress_tx(20),
    ];
    let batch = remote_batch(peer, transactions);
    let prepared = authority
        .plan_retained_admission_batch(&batch)
        .expect("the owner prefix and first pressure effect fit");
    assert_eq!(prepared.consumed(), 3);
    let committed = prepared.apply();
    assert_eq!(committed.consumed(), 3);
    drop(committed);
    assert_eq!(authority.owner_count(), 2);
    assert_eq!(authority.preaccepted_for_peer_for_reference(peer).len(), 2);
    assert!(authority.primary_projection_consistent());
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

    let committed = authority
        .plan_retained_admission_batch(&proposal_batch([proposal_variant]))
        .expect("the payload variant is an ordinary batch outcome")
        .apply();
    assert_eq!(committed.consumed(), 1);
    drop(committed);
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_retained_ingress_batch_plan_failure_and_drop_are_mutation_free() {
    let transactions = vec![ingress_tx(14), ingress_tx(15)];
    let mut dropped = TxPoolAuthority::for_foundation(limits());
    let before_drop = dropped.normalized_snapshot();
    let prepared = dropped
        .plan_retained_admission_batch(&proposal_batch(transactions.clone()))
        .expect("the batch plans without mutation");
    drop(prepared);
    assert_eq!(dropped.normalized_snapshot(), before_drop);

    let mut exhausted = TxPoolAuthority::for_foundation(limits());
    exhausted.force_next_version(EntryVersion(u128::MAX));
    let before_error = exhausted.normalized_snapshot();
    assert!(matches!(
        exhausted.plan_retained_admission_batch(&proposal_batch(transactions)),
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
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    let committed = authority
        .plan_retained_admission_batch(&batch)
        .expect("a no-owner proposal outcome is ordinary")
        .apply();

    assert_eq!(committed.consumed(), 2);
    drop(committed);
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_retained_ingress_batch_pressure_noop_does_not_require_an_apply_sequence() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let resident = (30..38).map(ingress_tx).collect::<Vec<_>>();
    drop(
        authority
            .plan_retained_admission_batch(&proposal_batch(resident))
            .expect("the fixture fills the total retained-owner envelope")
            .apply(),
    );
    assert_eq!(authority.owner_count(), 8);

    authority.force_next_sequence(ApplySequence(u128::MAX));
    let before = authority.normalized_snapshot();
    let committed = authority
        .plan_retained_admission_batch(&proposal_batch([ingress_tx(38)]))
        .expect("a proposal excluded by projected pressure performs no Apply")
        .apply();

    assert_eq!(committed.consumed(), 1);
    drop(committed);
    assert_eq!(authority.normalized_snapshot(), before);
}
