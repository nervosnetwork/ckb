//! Exhaustive finite refinement of the continuous resource algebra.
//!
//! The production side uses the real aggregate ledger, sealed charge variants
//! and Plan/Apply transition. The owner projection supplies the current
//! per-key charge exactly as production does. The reference side independently
//! recomputes usage from the final charge set; no production aggregate is
//! copied into the oracle.

use super::claim_relations::{
    ClaimComputeGrant, ContinuousAcceptedResources, ContinuousChargeRecord,
    ContinuousComputeLimits, ContinuousResourceChange, ContinuousResourceConfigError,
    ContinuousResourceLedger, ContinuousResourceLimits, ContinuousResourceUsage,
    ContinuousResourceVector,
};
use crate::authority::{
    resources::{
        AcceptedResources, ChargeRecord, ComputeGrant, ComputeLimits, ResidencyPolicy,
        ResourceConfigError, ResourceError, ResourceLedger, ResourceLimits, ResourceVector,
        test_support::TestResourceLedger,
    },
    state::RawTxHash,
};
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigurationObservation {
    Valid,
    InvalidConfiguration,
    ArithmeticOverflow,
}

fn production_key(key: u8) -> RawTxHash {
    RawTxHash(Byte32::new([key; 32]))
}

fn production_vector(vector: ContinuousResourceVector) -> ResourceVector {
    ResourceVector::new(
        usize::from(vector.entries),
        usize::from(vector.bytes),
        usize::from(vector.edges),
        usize::from(vector.active_work),
    )
    .with_compute_capacity(
        usize::from(vector.compute_bytes),
        usize::from(vector.compute_edges),
    )
    .expect("the finite refinement vector cannot overflow usize")
}

fn refinement_vector(vector: ResourceVector) -> ContinuousResourceVector {
    ContinuousResourceVector {
        entries: u16::try_from(vector.entries).expect("finite entries fit u16"),
        bytes: u16::try_from(vector.bytes).expect("finite bytes fit u16"),
        edges: u16::try_from(vector.edges).expect("finite edges fit u16"),
        active_work: u16::try_from(vector.active_work).expect("finite active work fits u16"),
        compute_bytes: u16::try_from(vector.compute_bytes()).expect("finite compute bytes fit u16"),
        compute_edges: u16::try_from(vector.compute_edges()).expect("finite compute edges fit u16"),
    }
}

fn production_accepted(resources: ContinuousAcceptedResources) -> AcceptedResources {
    AcceptedResources::new(
        usize::from(resources.entries),
        usize::from(resources.serialized_bytes),
        usize::from(resources.resident_bytes),
        u64::from(resources.cycles),
    )
}

fn refinement_accepted(resources: AcceptedResources) -> ContinuousAcceptedResources {
    ContinuousAcceptedResources {
        entries: u16::try_from(resources.entries).expect("finite accepted entries fit u16"),
        serialized_bytes: u16::try_from(resources.serialized_bytes)
            .expect("finite serialized bytes fit u16"),
        resident_bytes: u16::try_from(resources.resident_bytes)
            .expect("finite resident bytes fit u16"),
        cycles: u16::try_from(resources.cycles).expect("finite cycles fit u16"),
    }
}

fn production_charge(charge: ContinuousChargeRecord) -> ChargeRecord {
    match charge {
        ContinuousChargeRecord::PreAccepted {
            resources,
            residency_peer,
            compute_peer,
        } => ChargeRecord::PreAccepted {
            resources: production_vector(resources),
            residency_peer: residency_peer.map(|peer| PeerIndex::from(usize::from(peer))),
            compute_peer: compute_peer.map(|peer| PeerIndex::from(usize::from(peer))),
        },
        ContinuousChargeRecord::ReplacementHistory(resources) => {
            ChargeRecord::ReplacementHistory(production_vector(resources))
        }
        ContinuousChargeRecord::Accepted(resources) => {
            ChargeRecord::Accepted(production_accepted(resources))
        }
    }
}

fn production_limits(
    limits: ContinuousResourceLimits,
) -> Result<ResourceLimits, ResourceConfigError> {
    ResourceLimits::with_residency_policy(
        production_vector(limits.preaccepted),
        production_vector(limits.remote),
        production_vector(limits.per_peer),
        production_accepted(limits.accepted),
        ComputeLimits::new(
            usize::from(limits.compute.resolved_total_retained_bytes),
            usize::from(limits.compute.accepted_total_retained_bytes),
            usize::from(limits.compute.expanded_edges),
        ),
        ResidencyPolicy::foundation(),
    )?
    .with_replacement_history_limit(production_vector(limits.replacement_history))
}

fn claim_configuration_observation(
    result: Result<ContinuousResourceLimits, ContinuousResourceConfigError>,
) -> ConfigurationObservation {
    match result {
        Ok(_) => ConfigurationObservation::Valid,
        Err(ContinuousResourceConfigError::TransientComputeOverflow) => {
            ConfigurationObservation::ArithmeticOverflow
        }
        Err(
            ContinuousResourceConfigError::LimitHierarchy
            | ContinuousResourceConfigError::MissingComputeCapacity
            | ContinuousResourceConfigError::NonMonotonicComputeEnvelope,
        ) => ConfigurationObservation::InvalidConfiguration,
    }
}

fn production_configuration_observation(
    result: Result<ResourceLimits, ResourceConfigError>,
) -> ConfigurationObservation {
    match result {
        Ok(_) => ConfigurationObservation::Valid,
        Err(ResourceConfigError::TransientComputeOverflow) => {
            ConfigurationObservation::ArithmeticOverflow
        }
        Err(
            ResourceConfigError::LimitHierarchy
            | ResourceConfigError::MissingComputeCapacity
            | ResourceConfigError::NonMonotonicComputeEnvelope,
        ) => ConfigurationObservation::InvalidConfiguration,
    }
}

fn validate_claim_limits(
    limits: ContinuousResourceLimits,
) -> Result<ContinuousResourceLimits, ContinuousResourceConfigError> {
    ContinuousResourceLimits::validate(
        limits.preaccepted,
        limits.remote,
        limits.per_peer,
        limits.replacement_history,
        limits.accepted,
        limits.compute,
    )
}

fn base_limits(global_limit: u16) -> ContinuousResourceLimits {
    let compute = ContinuousComputeLimits {
        resolved_total_retained_bytes: 4,
        accepted_total_retained_bytes: 4,
        expanded_edges: 2,
    };
    let vector_limit = ContinuousResourceVector {
        entries: global_limit,
        bytes: global_limit * 2,
        edges: global_limit,
        active_work: global_limit,
        compute_bytes: global_limit * compute.max_total_retained_bytes(),
        compute_edges: global_limit * compute.expanded_edges,
    };
    ContinuousResourceLimits::validate(
        vector_limit,
        vector_limit,
        ContinuousResourceVector {
            entries: 1,
            bytes: 2,
            edges: 1,
            active_work: 1,
            compute_bytes: 4,
            compute_edges: 2,
        },
        ContinuousResourceVector::retained(1, 2, 1),
        ContinuousAcceptedResources {
            entries: 1,
            serialized_bytes: 2,
            resident_bytes: 2,
            cycles: 2,
        },
        compute,
    )
    .expect("the finite resource hierarchy is valid")
}

fn vector_domain() -> Vec<ContinuousResourceVector> {
    let mut vectors = Vec::with_capacity(3usize.pow(6));
    for encoded in 0..3usize.pow(6) {
        let mut value = encoded;
        let mut next = || {
            let digit = u16::try_from(value % 3).expect("a ternary digit fits u16");
            value /= 3;
            digit
        };
        vectors.push(ContinuousResourceVector {
            entries: next(),
            bytes: next(),
            edges: next(),
            active_work: next(),
            compute_bytes: next(),
            compute_edges: next(),
        });
    }
    vectors
}

fn accepted_domain() -> Vec<ContinuousAcceptedResources> {
    let mut vectors = Vec::with_capacity(3usize.pow(4));
    for encoded in 0..3usize.pow(4) {
        let mut value = encoded;
        let mut next = || {
            let digit = u16::try_from(value % 3).expect("a ternary digit fits u16");
            value /= 3;
            digit
        };
        vectors.push(ContinuousAcceptedResources {
            entries: next(),
            serialized_bytes: next(),
            resident_bytes: next(),
            cycles: next(),
        });
    }
    vectors
}

fn charge_domain() -> Vec<ContinuousChargeRecord> {
    let retained = ContinuousResourceVector::retained(1, 2, 1);
    let reserved = retained
        .reserve_compute(ClaimComputeGrant {
            total_retained_bytes: 4,
            edges: 2,
        })
        .expect("the retained charge accepts one compute grant");
    vec![
        ContinuousChargeRecord::PreAccepted {
            resources: retained,
            residency_peer: None,
            compute_peer: None,
        },
        ContinuousChargeRecord::PreAccepted {
            resources: reserved,
            residency_peer: None,
            compute_peer: None,
        },
        ContinuousChargeRecord::PreAccepted {
            resources: retained,
            residency_peer: Some(1),
            compute_peer: None,
        },
        ContinuousChargeRecord::PreAccepted {
            resources: reserved,
            residency_peer: Some(1),
            compute_peer: Some(1),
        },
        ContinuousChargeRecord::PreAccepted {
            resources: retained,
            residency_peer: Some(2),
            compute_peer: None,
        },
        ContinuousChargeRecord::PreAccepted {
            resources: reserved,
            residency_peer: Some(2),
            compute_peer: Some(2),
        },
        ContinuousChargeRecord::ReplacementHistory(retained),
        ContinuousChargeRecord::Accepted(ContinuousAcceptedResources {
            entries: 1,
            serialized_bytes: 2,
            resident_bytes: 2,
            cycles: 2,
        }),
    ]
}

fn production_ledger(
    limits: ContinuousResourceLimits,
    charges: &BTreeMap<u8, ContinuousChargeRecord>,
) -> Result<TestResourceLedger, ResourceError> {
    let mut ledger = TestResourceLedger::new(
        production_limits(limits).expect("the transition fixture limits are valid"),
    );
    let changes = charges
        .iter()
        .map(|(key, charge)| (production_key(*key), None, Some(production_charge(*charge))))
        .collect();
    let plan = ledger.plan_batch(changes)?;
    ledger.apply_batch(plan);
    Ok(ledger)
}

fn production_usage(ledger: &TestResourceLedger) -> ContinuousResourceUsage {
    let snapshot = ledger.snapshot();
    let per_peer = snapshot
        .peers
        .into_iter()
        .map(|(peer, resources)| {
            (
                u8::try_from(peer.value()).expect("the finite peer fits u8"),
                refinement_vector(resources),
            )
        })
        .collect();
    ContinuousResourceUsage {
        preaccepted: refinement_vector(snapshot.preaccepted),
        remote: refinement_vector(snapshot.remote),
        per_peer,
        replacement_history: refinement_vector(snapshot.replacement_history),
        accepted: refinement_accepted(snapshot.accepted),
    }
}

fn assert_ledger_matches_claim(production: &TestResourceLedger, claim: &ContinuousResourceLedger) {
    let usage = production_usage(production);
    assert_eq!(
        usage,
        claim.usage().expect("a planned claim ledger is valid")
    );
}

#[test]
fn uak_resource_vectors_and_configuration_refine_the_finite_algebra_exhaustively() {
    let vectors = vector_domain();
    let grant = ClaimComputeGrant {
        total_retained_bytes: 2,
        edges: 1,
    };
    let production_grant = ComputeGrant::for_foundation(2, 1);
    let compute = base_limits(2).compute;
    let admission_ledger = ResourceLedger::new(
        production_limits(base_limits(2)).expect("the admission fixture is configured"),
    );
    for vector in &vectors {
        let production = production_vector(*vector);
        assert_eq!(refinement_vector(production), *vector);
        assert_eq!(
            production.total_bytes().map(|value| value as u16),
            vector.total_bytes()
        );
        assert_eq!(
            production.total_edges().map(|value| value as u16),
            vector.total_edges()
        );
        assert_eq!(
            production
                .reserve_compute(production_grant)
                .map(refinement_vector),
            vector.reserve_compute(grant)
        );
        assert_eq!(
            refinement_vector(production.without_compute()),
            vector.without_compute()
        );
        assert_eq!(
            admission_ledger.validate_admission(production).is_ok(),
            compute.admits(*vector),
            "compute admission mismatch for {vector:?}"
        );
    }
    for left in &vectors {
        let production_left = production_vector(*left);
        for right in &vectors {
            let production_right = production_vector(*right);
            assert_eq!(
                production_left
                    .checked_add(production_right)
                    .map(refinement_vector),
                left.checked_add(*right)
            );
            assert_eq!(
                production_left
                    .checked_sub(production_right)
                    .map(refinement_vector),
                left.checked_sub(*right)
            );
            assert_eq!(production_left.fits(production_right), left.fits(*right));
        }
    }

    let accepted = accepted_domain();
    for left in &accepted {
        let production_left = production_accepted(*left);
        for right in &accepted {
            let production_right = production_accepted(*right);
            assert_eq!(
                production_left
                    .checked_add(production_right)
                    .map(refinement_accepted),
                left.checked_add(*right)
            );
            assert_eq!(
                production_left
                    .checked_sub(production_right)
                    .map(refinement_accepted),
                left.checked_sub(*right)
            );
            assert_eq!(production_left.fits(production_right), left.fits(*right));
        }
    }

    let baseline = ContinuousResourceLimits {
        preaccepted: ContinuousResourceVector {
            entries: 2,
            bytes: 8,
            edges: 4,
            active_work: 2,
            compute_bytes: 8,
            compute_edges: 4,
        },
        remote: ContinuousResourceVector {
            entries: 2,
            bytes: 8,
            edges: 4,
            active_work: 2,
            compute_bytes: 8,
            compute_edges: 4,
        },
        per_peer: ContinuousResourceVector {
            entries: 1,
            bytes: 4,
            edges: 2,
            active_work: 1,
            compute_bytes: 4,
            compute_edges: 2,
        },
        replacement_history: ContinuousResourceVector::retained(1, 4, 2),
        accepted: ContinuousAcceptedResources {
            entries: 2,
            serialized_bytes: 8,
            resident_bytes: 8,
            cycles: 8,
        },
        compute: ContinuousComputeLimits {
            resolved_total_retained_bytes: 3,
            accepted_total_retained_bytes: 4,
            expanded_edges: 2,
        },
    };
    for mask in 0u16..(1u16 << 13) {
        let mut candidate = baseline;
        if mask & (1 << 0) != 0 {
            candidate.remote.entries = candidate.preaccepted.entries + 1;
        }
        if mask & (1 << 1) != 0 {
            candidate.per_peer.entries = candidate.remote.entries + 1;
        }
        if mask & (1 << 2) != 0 {
            candidate.replacement_history.entries = candidate.preaccepted.entries + 1;
        }
        if mask & (1 << 3) != 0 {
            candidate.replacement_history.active_work = 1;
        }
        if mask & (1 << 4) != 0 {
            candidate.compute.resolved_total_retained_bytes = 0;
        }
        if mask & (1 << 5) != 0 {
            candidate.compute.accepted_total_retained_bytes = 0;
        }
        if mask & (1 << 6) != 0 {
            candidate.preaccepted.active_work = 0;
        }
        if mask & (1 << 7) != 0 {
            candidate.remote.active_work = 0;
        }
        if mask & (1 << 8) != 0 {
            candidate.per_peer.active_work = 0;
        }
        if mask & (1 << 9) != 0 {
            candidate.compute.resolved_total_retained_bytes = 5;
        }
        if mask & (1 << 10) != 0 {
            candidate.preaccepted.compute_bytes = 3;
        }
        if mask & (1 << 11) != 0 {
            candidate.remote.compute_edges = 1;
        }
        if mask & (1 << 12) != 0 {
            candidate.per_peer.compute_bytes = 3;
        }
        assert_eq!(
            production_configuration_observation(production_limits(candidate)),
            claim_configuration_observation(validate_claim_limits(candidate)),
            "configuration premise mask {mask:#x}"
        );
    }

    let claim_overflow = ContinuousResourceLimits::validate(
        ContinuousResourceVector {
            entries: 1,
            bytes: u16::MAX,
            edges: 1,
            active_work: 1,
            compute_bytes: 1,
            compute_edges: 1,
        },
        ContinuousResourceVector {
            entries: 1,
            bytes: u16::MAX,
            edges: 1,
            active_work: 1,
            compute_bytes: 1,
            compute_edges: 1,
        },
        ContinuousResourceVector {
            entries: 1,
            bytes: u16::MAX,
            edges: 1,
            active_work: 1,
            compute_bytes: 1,
            compute_edges: 1,
        },
        ContinuousResourceVector::default(),
        ContinuousAcceptedResources::default(),
        ContinuousComputeLimits {
            resolved_total_retained_bytes: 1,
            accepted_total_retained_bytes: 1,
            expanded_edges: 1,
        },
    );
    let production_overflow = ResourceLimits::new(
        ResourceVector::new(1, usize::MAX, 1, 1),
        ResourceVector::new(1, usize::MAX, 1, 1),
        ResourceVector::new(1, usize::MAX, 1, 1),
        AcceptedResources::default(),
        ComputeLimits::new(1, 1, 1),
    );
    assert_eq!(
        claim_configuration_observation(claim_overflow),
        production_configuration_observation(production_overflow)
    );
}

#[test]
fn uak_resource_charge_and_batch_refine_the_finite_set_transition_exhaustively() {
    let wide = ContinuousResourceLimits::validate(
        ContinuousResourceVector {
            entries: 16,
            bytes: 32,
            edges: 16,
            active_work: 16,
            compute_bytes: 32,
            compute_edges: 32,
        },
        ContinuousResourceVector {
            entries: 16,
            bytes: 32,
            edges: 16,
            active_work: 16,
            compute_bytes: 32,
            compute_edges: 32,
        },
        ContinuousResourceVector {
            entries: 16,
            bytes: 32,
            edges: 16,
            active_work: 16,
            compute_bytes: 32,
            compute_edges: 32,
        },
        ContinuousResourceVector::retained(16, 32, 16),
        ContinuousAcceptedResources {
            entries: 16,
            serialized_bytes: 32,
            resident_bytes: 32,
            cycles: 32,
        },
        ContinuousComputeLimits {
            resolved_total_retained_bytes: 1,
            accepted_total_retained_bytes: 1,
            expanded_edges: 1,
        },
    )
    .expect("the validation fixture is wide");
    for entries in 0..=2 {
        for active_work in 0..=2 {
            for compute_bytes in 0..=2 {
                for compute_edges in 0..=2 {
                    for residency_peer in [None, Some(1)] {
                        for compute_peer in [None, Some(1), Some(2)] {
                            let claim = ContinuousChargeRecord::PreAccepted {
                                resources: ContinuousResourceVector {
                                    entries,
                                    bytes: 1,
                                    edges: 1,
                                    active_work,
                                    compute_bytes,
                                    compute_edges,
                                },
                                residency_peer,
                                compute_peer,
                            };
                            let production = production_ledger(wide, &BTreeMap::from([(0, claim)]));
                            assert_eq!(
                                production.is_ok(),
                                claim.validate().is_ok(),
                                "charge validation mismatch for {claim:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    for entries in 0..=2 {
        for active_work in 0..=2 {
            for compute_bytes in 0..=2 {
                for compute_edges in 0..=2 {
                    let claim =
                        ContinuousChargeRecord::ReplacementHistory(ContinuousResourceVector {
                            entries,
                            bytes: 1,
                            edges: 1,
                            active_work,
                            compute_bytes,
                            compute_edges,
                        });
                    assert_eq!(
                        production_ledger(wide, &BTreeMap::from([(0, claim)])).is_ok(),
                        claim.validate().is_ok(),
                        "history validation mismatch for {claim:?}"
                    );
                }
            }
        }
    }

    let records = charge_domain();
    let mut options = Vec::with_capacity(records.len() + 1);
    options.push(None);
    options.extend(records.iter().copied().map(Some));
    for limits in [base_limits(2), base_limits(1)] {
        for initial_left in &options {
            for initial_right in &options {
                let initial = [*initial_left, *initial_right];
                let initial_map = initial
                    .iter()
                    .copied()
                    .enumerate()
                    .filter_map(|(key, charge)| {
                        charge.map(|charge| {
                            (
                                u8::try_from(key).expect("the two-key domain fits u8"),
                                charge,
                            )
                        })
                    })
                    .collect::<BTreeMap<_, _>>();
                let claim = ContinuousResourceLedger::new(limits, initial_map.clone());
                let production = production_ledger(limits, &initial_map);
                assert_eq!(production.is_ok(), claim.is_ok());
                let (Ok(claim), Ok(_)) = (claim, production) else {
                    continue;
                };
                for after_left in &options {
                    for after_right in &options {
                        let after = [*after_left, *after_right];
                        let changes = [
                            ContinuousResourceChange {
                                key: 0,
                                expected: initial[0],
                                after: after[0],
                            },
                            ContinuousResourceChange {
                                key: 1,
                                expected: initial[1],
                                after: after[1],
                            },
                        ];
                        let expected = claim.plan_changes(&changes);
                        let mut actual = production_ledger(limits, &initial_map)
                            .expect("the initial production ledger was already validated");
                        let before = actual.snapshot();
                        let plan = actual.plan_batch(
                            changes
                                .iter()
                                .map(|change| {
                                    (
                                        production_key(change.key),
                                        change.expected.map(production_charge),
                                        change.after.map(production_charge),
                                    )
                                })
                                .collect(),
                        );
                        assert_eq!(plan.is_ok(), expected.is_ok());
                        match (plan, expected) {
                            (Ok(plan), Ok(expected)) => {
                                actual.apply_batch(plan);
                                assert_ledger_matches_claim(&actual, &expected);

                                let mut reversed = production_ledger(limits, &initial_map)
                                    .expect("the reverse fixture starts valid");
                                let reverse_plan = reversed
                                    .plan_batch(
                                        changes
                                            .iter()
                                            .rev()
                                            .map(|change| {
                                                (
                                                    production_key(change.key),
                                                    change.expected.map(production_charge),
                                                    change.after.map(production_charge),
                                                )
                                            })
                                            .collect(),
                                    )
                                    .expect("the same set transition is order independent");
                                reversed.apply_batch(reverse_plan);
                                assert_eq!(actual.snapshot(), reversed.snapshot());
                            }
                            (Err(_), Err(_)) => assert_eq!(actual.snapshot(), before),
                            _ => unreachable!("success parity was checked above"),
                        }
                    }
                }
            }
        }
    }
}
