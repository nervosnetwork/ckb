use super::{CommittedPublicReject, ComponentLimitKind, MembershipReject};
use crate::constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES;
use crate::error::Reject;
use ckb_types::{
    core::{Capacity, FeeRate, error::OutPointError},
    packed::{Byte32, OutPoint},
};

fn out_point(byte: u8) -> OutPoint {
    OutPoint::new(Byte32::new([byte; 32]), 0)
}

#[test]
fn membership_rejection_compiler_preserves_public_rule_semantics() {
    let occupied = out_point(1);
    assert!(matches!(
        MembershipReject::InputConflict(occupied.clone()).into_public(),
        Reject::Resolve(OutPointError::Dead(actual)) if actual == occupied
    ));

    assert!(matches!(
        MembershipReject::ComponentLimit {
            kind: ComponentLimitKind::Replacement,
            limit: 100,
        }
        .into_public(),
        Reject::RBFRejected(message)
            if message.contains(">= 101") && message.contains("<= 100")
    ));
    assert!(matches!(
        MembershipReject::ComponentLimit {
            kind: ComponentLimitKind::Mutation,
            limit: 100,
        }
        .into_public(),
        Reject::Full(message)
            if message == "pool mutation exceeds the per-transition limit of 100"
    ));

    let fee_rate = FeeRate::from_u64(42);
    assert!(matches!(
        MembershipReject::CandidateEvicted { fee_rate }.into_public(),
        Reject::Full(message)
            if message == format!("the fee_rate for this transaction is: {fee_rate}")
    ));
    let actual = Capacity::shannons(10);
    let required = Capacity::shannons(11);
    assert!(matches!(
        MembershipReject::InsufficientReplacementFee {
            actual,
            required,
        }
        .into_public(),
        Reject::RBFRejected(message)
            if message == format!(
                "Tx's current fee is {actual}, expect it to >= {required} to replace old txs"
            )
    ));
}

#[test]
fn membership_recent_reject_classification_matches_the_public_policy() {
    let cases = vec![
        MembershipReject::InputConflict(out_point(1)),
        MembershipReject::TooManyAncestors,
        MembershipReject::ComponentLimit {
            kind: ComponentLimitKind::Replacement,
            limit: 100,
        },
        MembershipReject::ComponentLimit {
            kind: ComponentLimitKind::Mutation,
            limit: 100,
        },
        MembershipReject::NewUnconfirmedInput(out_point(2)),
        MembershipReject::InputFromDescendant(out_point(3)),
        MembershipReject::AncestorDescendantOverlap,
        MembershipReject::DependencyOnVictim(out_point(4)),
        MembershipReject::InsufficientReplacementFee {
            actual: Capacity::shannons(10),
            required: Capacity::shannons(11),
        },
        MembershipReject::ReplacementFeeOverflow,
        MembershipReject::AggregateOverflow,
        MembershipReject::CandidateEvicted {
            fee_rate: FeeRate::from_u64(42),
        },
        MembershipReject::CausalCycle(super::RawTxHash(Byte32::new([5; 32]))),
        MembershipReject::MissingInputEvidence(out_point(6)),
        MembershipReject::MissingDependencyEvidence(out_point(7)),
        MembershipReject::MissingPoolOutput(out_point(8)),
    ];

    for reason in cases {
        assert_eq!(
            reason.should_record_recent_reject(),
            reason.clone().into_public().should_recorded(),
            "allocation-free effect classification drifted for {reason:?}"
        );
    }
}

#[test]
fn committed_public_rejection_bounds_diagnostics_without_changing_policy() {
    let original = Reject::RBFRejected(
        "x".repeat(
            MAX_TX_POOL_REJECT_DESCRIPTION_BYTES
                .checked_mul(2)
                .expect("fixture length fits"),
        ),
    );
    let committed = CommittedPublicReject::new(original);
    assert!(matches!(committed.reject(), Reject::RBFRejected(_)));
    assert!(committed.description_bytes() <= MAX_TX_POOL_REJECT_DESCRIPTION_BYTES);
    assert!(!committed.is_malformed());
    assert!(committed.should_record());
    assert!(committed.relay_allowed());
}

#[test]
fn committed_public_rejection_detaches_spare_string_capacity() {
    let mut diagnostic = String::with_capacity(
        MAX_TX_POOL_REJECT_DESCRIPTION_BYTES
            .checked_mul(4)
            .expect("fixture capacity fits"),
    );
    diagnostic.push_str("short transient diagnostic");
    let committed = CommittedPublicReject::new(Reject::Full(diagnostic));

    let Reject::Full(diagnostic) = committed.reject() else {
        panic!("the rejection variant must be preserved");
    };
    assert_eq!(diagnostic, "short transient diagnostic");
    assert!(diagnostic.capacity() <= MAX_TX_POOL_REJECT_DESCRIPTION_BYTES);
}
