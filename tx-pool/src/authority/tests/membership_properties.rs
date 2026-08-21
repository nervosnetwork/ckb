//! Pointwise refinement of the bounded membership frontier.
//!
//! The shared module supplies only symbolic cases and claim observations.
//! This file independently constructs production transactions, receipts and
//! authority state, then calls the production settlement planner.

use super::claim_relations::{
    CellRole, ClaimFeeRate, ClaimMinimumFeeObservation, ClaimTransactionCost, EffectPressure,
    EvictionRefinementInput, EvictionRefinementMetrics, EvictionRefinementObservation,
    EvictionRefinementStatus, EvidenceOriginRole, FrontierObservation, FrontierTerminal,
    REFINEMENT_MAX_READY, SourceRole, accepted_capacity_observation, accepted_role_observation,
    candidate_graph_observation, candidate_role_observation, eviction_observation,
    eviction_status_witness, evidence_origin_observation, minimum_fee_observation,
    positioned_role_observation, ready_order_observation, shared_header_observation,
    source_observation, source_pressure_observation, stale_observation,
};
use super::foundation::{
    accept_remote_transaction_with_payload, accept_remote_transaction_with_payload_and_cycles,
    apply_plan, independent_batch, resolved_payload_with_facts,
    verify_remote_transaction_with_payload,
};
use crate::{
    authority::{
        chain::ProposalContextReceipt,
        effect::{CommittedAcceptance, CommittedEffect, EffectPolicy},
        plan::{Backpressure, PlanError, SettlementPlan, TxPoolAuthority},
        resources::{AcceptedResources, ComputeLimits, ResourceLimits, ResourceVector},
        state::{
            AcceptedStatus, CandidateMetrics, ChainRevision, ChainViewId, OwnedTx,
            PreAcceptedPhase, RawTxHash, ResolvedPayload, ValidatedAdmission,
        },
    },
    constants::MAX_READY_BATCH,
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, FeeRate, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};

const RANKED_FEES: [u64; REFINEMENT_MAX_READY] = [
    1_000_000_000_000_000_000,
    10_000_000_000_000_000,
    100_000_000_000_000,
    1_000_000_000_000,
    10_000_000_000,
    100_000_000,
    1_000_000,
    10_000,
];

#[test]
fn uak_configured_fee_rate_refines_production_arithmetic_pointwise() {
    let rates = [0, 1, 999, 1_000, 1_500, 2_000, u64::MAX];
    let weights = [0, 1, 2, 999, 1_000, 1_001, u32::MAX as u64, u64::MAX];

    for rate in rates {
        let production = FeeRate::from_u64(rate);
        let claim = ClaimFeeRate::from_u64(rate);
        assert_eq!(production.as_u64(), claim.as_u64());

        for weight in weights {
            let production_required = production.fee(weight).as_u64();
            let claim_required = claim.fee(weight);
            assert_eq!(
                production_required, claim_required,
                "configured fee rate {rate} and weight {weight}"
            );

            for actual in [
                production_required.saturating_sub(1),
                production_required,
                production_required.saturating_add(1),
            ] {
                let expected = if actual < production_required {
                    ClaimMinimumFeeObservation::Rejected {
                        actual,
                        required: production_required,
                    }
                } else {
                    ClaimMinimumFeeObservation::Accepted {
                        actual,
                        required: production_required,
                    }
                };
                assert_eq!(minimum_fee_observation(actual, weight, claim), expected);
            }
        }
    }
}

#[test]
fn uak_candidate_role_product_matches_the_claim_relation_pointwise() {
    for left in CellRole::ALL {
        for right in CellRole::ALL {
            let expected = candidate_role_observation(left, right);
            let actual = production_candidate_roles(&[left, right]);
            assert_eq!(actual, expected, "candidate roles {left:?}/{right:?}");
        }
    }
}

#[test]
fn uak_candidate_accepted_role_product_matches_the_claim_relation_pointwise() {
    for candidate in CellRole::ALL {
        for accepted in CellRole::ALL {
            let expected = accepted_role_observation(candidate, accepted);
            let actual = production_accepted_roles(candidate, accepted);
            assert_eq!(
                actual, expected,
                "candidate/Accepted roles {candidate:?}/{accepted:?}"
            );
        }
    }
}

#[test]
fn uak_every_four_owner_coupling_graph_matches_the_claim_relation_prefix() {
    let mut observations = Vec::with_capacity(64);
    for edge_mask in 0u8..64 {
        let expected = candidate_graph_observation(edge_mask);
        let actual = production_shared_input_graph(edge_mask);
        assert_eq!(actual, expected, "four-owner edge mask {edge_mask:06b}");
        observations.push(actual);
    }
    for subset in 0u8..64 {
        for superset in 0u8..64 {
            if subset & superset != subset {
                continue;
            }
            assert!(
                observations[usize::from(superset)].prefix_len
                    <= observations[usize::from(subset)].prefix_len,
                "production edge superset {superset:06b} extended subset {subset:06b}"
            );
        }
    }
}

#[test]
fn uak_every_single_coupled_edge_position_matches_the_claim_relation_prefix() {
    assert_eq!(REFINEMENT_MAX_READY, MAX_READY_BATCH);
    for earlier_role in CellRole::ALL {
        for later_role in CellRole::ALL {
            let pair = candidate_role_observation(earlier_role, later_role);
            if pair.terminal != FrontierTerminal::Coupled {
                continue;
            }
            for earlier in 0..REFINEMENT_MAX_READY {
                for later in (earlier + 1)..REFINEMENT_MAX_READY {
                    let expected = positioned_role_observation(
                        REFINEMENT_MAX_READY,
                        earlier,
                        earlier_role,
                        later,
                        later_role,
                    );
                    let mut roles = vec![CellRole::None; REFINEMENT_MAX_READY];
                    roles[earlier] = earlier_role;
                    roles[later] = later_role;
                    let actual = production_candidate_roles(&roles);
                    assert_eq!(
                        actual, expected,
                        "positioned roles {earlier_role:?}@{earlier}/{later_role:?}@{later}"
                    );
                }
            }
        }
    }
}

#[test]
fn uak_source_control_classes_match_the_claim_relation_prefix() {
    let cases = [
        vec![SourceRole::Trusted],
        vec![SourceRole::Remote],
        vec![SourceRole::Trusted, SourceRole::Remote],
        vec![SourceRole::Remote, SourceRole::Trusted],
        vec![SourceRole::Trusted, SourceRole::Remote, SourceRole::Remote],
        vec![SourceRole::Remote, SourceRole::Trusted, SourceRole::Trusted],
    ];
    for sources in cases {
        assert_eq!(
            production_source_roles(&sources),
            source_observation(&sources),
            "source-control case {sources:?}"
        );
    }
}

#[test]
fn uak_accepted_entry_capacity_refines_every_ready_prefix() {
    for candidate_count in 1..=REFINEMENT_MAX_READY {
        for accepted_entries in 1..=REFINEMENT_MAX_READY {
            let accepted_entries =
                u16::try_from(accepted_entries).expect("the finite Accepted capacity fits u16");
            assert_eq!(
                production_accepted_capacity(candidate_count, accepted_entries),
                accepted_capacity_observation(candidate_count, accepted_entries),
                "Accepted entries {accepted_entries}, candidates {candidate_count}"
            );
        }
    }
}

#[test]
fn uak_partitioned_effect_pressure_refines_source_control() {
    for source in [SourceRole::Trusted, SourceRole::Remote] {
        for pressure in [EffectPressure::RemoteFull, EffectPressure::OrdinaryFull] {
            assert_eq!(
                production_source_pressure(source, pressure),
                source_pressure_observation(source, pressure),
                "effect pressure {pressure:?} for {source:?}"
            );
        }
    }
}

#[test]
fn uak_stale_ready_evidence_matches_the_claim_relation_terminal() {
    assert_eq!(production_stale_observation(), stale_observation());
}

#[test]
fn uak_shared_headers_refine_as_commutative_reads() {
    for owner_count in 1..=REFINEMENT_MAX_READY {
        assert_eq!(
            production_shared_header_observation(owner_count),
            shared_header_observation(owner_count),
            "shared-header owner count {owner_count}"
        );
    }
}

#[test]
fn uak_ready_cost_quotient_rejects_payload_serialized_alias() {
    let transactions = [
        ready_order_transaction(0, 0),
        ready_order_transaction(1, 512),
    ];
    let payload_bytes = transactions.each_ref().map(|transaction| {
        u64::try_from(transaction.data().total_size())
            .expect("the finite transaction size fits u64")
    });
    assert!(payload_bytes[0] < payload_bytes[1]);

    // The mediant between p0/p1 and (p0+4)/(p1+4) is a constructive
    // counterexample: raw-payload fee rate selects 0, while the protocol's
    // block-serialized fee rate selects 1.
    let fees = payload_bytes.map(|bytes| {
        bytes
            .checked_mul(2)
            .and_then(|value| value.checked_add(4))
            .expect("the finite alias witness fits u64")
    });
    assert!(
        u128::from(fees[0]) * u128::from(payload_bytes[1])
            > u128::from(fees[1]) * u128::from(payload_bytes[0])
    );
    let serialized_bytes = payload_bytes.map(|bytes| bytes + 4);
    assert!(
        u128::from(fees[0]) * u128::from(serialized_bytes[1])
            < u128::from(fees[1]) * u128::from(serialized_bytes[0])
    );

    let (production, costs) = production_ready_observation(&transactions, &fees);
    let expected = ready_order_observation(&costs);
    assert_eq!(expected, vec![1, 0]);
    assert_eq!(production, expected);
}

#[test]
fn uak_chain_and_pool_evidence_origins_refine_pointwise() {
    for origin in [
        EvidenceOriginRole::ChainInput,
        EvidenceOriginRole::ChainRead,
        EvidenceOriginRole::PoolInput,
        EvidenceOriginRole::PoolRead,
    ] {
        assert_eq!(
            production_evidence_origin(origin),
            evidence_origin_observation(origin),
            "evidence origin {origin:?}"
        );
    }
}

#[test]
fn uak_eviction_order_refines_the_exact_ckb_weight_and_tuple() {
    let authority = production_eviction_fixture();
    let snapshot = authority.membership_snapshot_for_reference();
    let mut expected = authority
        .entries_for_reference()
        .iter()
        .filter_map(|(hash, owner)| {
            let OwnedTx::Accepted(entry) = owner else {
                return None;
            };
            let aggregate = snapshot.descendant_aggregates.get(hash)?;
            let own = production_cost_receipt(&entry.record.tx, entry.proof.metrics())?;
            Some(eviction_observation(EvictionRefinementInput {
                status: production_eviction_status_receipt(&entry.proposal),
                own: EvictionRefinementMetrics::new(
                    own.fee(),
                    u64::from(own.serialized_bytes()),
                    own.cycles(),
                ),
                descendants: EvictionRefinementMetrics::new(
                    aggregate.fee.as_u64(),
                    u64::try_from(aggregate.serialized_bytes).ok()?,
                    aggregate.cycles,
                ),
                descendants_count: aggregate.entries,
                arrival: entry.record.arrival.0,
                identity: refinement_identity(hash),
            }))
        })
        .collect::<Vec<_>>();
    expected.sort_unstable();

    let actual = snapshot
        .eviction_order
        .iter()
        .map(|key| EvictionRefinementObservation {
            status: refinement_status(key.status),
            fee_rate: key.fee_rate.as_u64(),
            descendants_count: key.descendants_count,
            arrival: key.arrival.0,
            identity: refinement_identity(&key.hash),
        })
        .collect::<Vec<_>>();

    assert_eq!(actual, expected);
    assert!(
        actual
            .iter()
            .any(|key| key.status == EvictionRefinementStatus::Pending)
            && actual
                .iter()
                .any(|key| key.status == EvictionRefinementStatus::Gap)
            && actual
                .iter()
                .any(|key| key.status == EvictionRefinementStatus::Proposed),
        "the production fixture covers every eviction-status rank"
    );
    assert!(
        actual.iter().any(|key| key.descendants_count > 1),
        "the production fixture covers descendant aggregates"
    );
}

fn production_candidate_roles(roles: &[CellRole]) -> FrontierObservation {
    if roles
        .iter()
        .filter(|role| **role == CellRole::Output)
        .count()
        > 1
    {
        assert_duplicate_output_premise(roles);
        return FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::DuplicateOutputIdentity,
        };
    }
    let transactions = transactions_for_roles(roles, 200);
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let hashes = transactions
        .into_iter()
        .enumerate()
        .map(|(index, transaction)| {
            verify_proposal(&mut authority, transaction, index, ranked_fee(index))
        })
        .collect::<Vec<_>>();
    observe_production_prefix(&mut authority, &hashes)
}

fn production_accepted_roles(
    candidate_role: CellRole,
    accepted_role: CellRole,
) -> FrontierObservation {
    if candidate_role == CellRole::Output && accepted_role == CellRole::Output {
        assert_duplicate_output_premise(&[candidate_role, accepted_role]);
        return FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::DuplicateOutputIdentity,
        };
    }
    let mut transactions = transactions_for_roles(&[candidate_role, accepted_role], 201);
    let accepted_transaction = transactions
        .pop()
        .expect("the accepted-role case contains an Accepted transaction");
    let candidate_transaction = transactions
        .pop()
        .expect("the accepted-role case contains a candidate transaction");
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let accepted_payload = resolved_payload_with_facts(
        &accepted_transaction,
        Vec::new(),
        accepted_transaction.input_pts_iter().collect(),
        Capacity::shannons(1_000),
    );
    accept_remote_transaction_with_payload(
        &mut authority,
        accepted_transaction,
        900,
        AcceptedStatus::Pending,
        accepted_payload,
    );
    let candidate = verify_proposal(&mut authority, candidate_transaction, 0, ranked_fee(0));
    observe_production_prefix(&mut authority, &[candidate])
}

fn production_shared_input_graph(edge_mask: u8) -> FrontierObservation {
    let owner_count = 4usize;
    let mut inputs = vec![Vec::new(); owner_count];
    let mut bit = 0u8;
    for left in 0..owner_count {
        for right in (left + 1)..owner_count {
            if edge_mask & (1u8 << bit) != 0 {
                let cell = OutPoint::new(Byte32::new([100u8.saturating_add(bit); 32]), 0);
                inputs[left].push(cell.clone());
                inputs[right].push(cell);
            }
            bit = bit.saturating_add(1);
        }
    }
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let mut hashes = Vec::with_capacity(owner_count);
    for (index, chain_inputs) in inputs.into_iter().enumerate() {
        let mut builder = TransactionBuilder::default().version(
            10_000u32
                .checked_add(u32::try_from(index).expect("finite graph index fits u32"))
                .expect("finite graph version is bounded"),
        );
        for input in &chain_inputs {
            builder = builder.input(CellInput::new(input.clone(), 0));
        }
        let transaction = builder.build();
        let payload = resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            chain_inputs,
            Capacity::shannons(ranked_fee(index)),
        );
        hashes.push(verify_proposal_with_payload(
            &mut authority,
            transaction,
            index,
            payload,
        ));
    }
    observe_production_prefix(&mut authority, &hashes)
}

fn production_source_roles(sources: &[SourceRole]) -> FrontierObservation {
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let hashes = sources
        .iter()
        .copied()
        .enumerate()
        .map(|(index, source)| {
            let transaction = transaction_for_role(index, CellRole::None, None);
            let payload = resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                Vec::new(),
                Capacity::shannons(ranked_fee(index)),
            );
            match source {
                SourceRole::Trusted => {
                    verify_proposal_with_payload(&mut authority, transaction, index, payload)
                }
                SourceRole::Remote => verify_remote_transaction_with_payload(
                    &mut authority,
                    transaction,
                    1_000 + index,
                    payload,
                ),
            }
        })
        .collect::<Vec<_>>();
    observe_production_prefix(&mut authority, &hashes)
}

fn production_evidence_origin(origin: EvidenceOriginRole) -> FrontierObservation {
    let candidate_role = match origin {
        EvidenceOriginRole::ChainInput | EvidenceOriginRole::PoolInput => CellRole::Input,
        EvidenceOriginRole::ChainRead | EvidenceOriginRole::PoolRead => CellRole::Read,
    };
    let pool_origin = matches!(
        origin,
        EvidenceOriginRole::PoolInput | EvidenceOriginRole::PoolRead
    );
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let mut transactions = transactions_for_roles(&[candidate_role, CellRole::Output], 203);
    let parent = transactions
        .pop()
        .expect("the evidence-origin fixture has one parent");
    let candidate = transactions
        .pop()
        .expect("the evidence-origin fixture has one candidate");
    if pool_origin {
        let parent_payload =
            resolved_payload_with_facts(&parent, Vec::new(), Vec::new(), Capacity::shannons(1_000));
        accept_remote_transaction_with_payload(
            &mut authority,
            parent,
            900,
            AcceptedStatus::Pending,
            parent_payload,
        );
    }
    let shared = match candidate_role {
        CellRole::Input => candidate
            .input_pts_iter()
            .next()
            .expect("the input-origin fixture has one cell"),
        CellRole::Read => candidate
            .cell_deps()
            .into_iter()
            .next()
            .expect("the read-origin fixture has one cell")
            .out_point(),
        CellRole::None | CellRole::Output => unreachable!("origin role is input or read"),
    };
    let (chain_inputs, chain_dependencies) = if pool_origin {
        (Vec::new(), Vec::new())
    } else {
        match candidate_role {
            CellRole::Input => (vec![shared], Vec::new()),
            CellRole::Read => (Vec::new(), vec![shared]),
            CellRole::None | CellRole::Output => unreachable!("origin role is input or read"),
        }
    };
    let payload = ResolvedPayload::for_foundation(
        &candidate,
        Vec::new(),
        64,
        Capacity::shannons(ranked_fee(0)),
        candidate.data().total_size(),
        chain_inputs,
        chain_dependencies,
    )
    .expect("the finite origin evidence matches the transaction shape");
    let hash = verify_proposal_with_payload(&mut authority, candidate, 0, payload);
    observe_production_prefix(&mut authority, &[hash])
}

fn production_accepted_capacity(
    candidate_count: usize,
    accepted_entries: u16,
) -> FrontierObservation {
    let limits = refinement_limits().with_accepted_for_foundation(AcceptedResources::new(
        usize::from(accepted_entries),
        256 * 1024,
        256 * 1024,
        256,
    ));
    let mut authority = TxPoolAuthority::for_foundation(limits);
    let hashes = transactions_for_roles(&vec![CellRole::None; candidate_count], 202)
        .into_iter()
        .enumerate()
        .map(|(index, transaction)| {
            verify_proposal(&mut authority, transaction, index, ranked_fee(index))
        })
        .collect::<Vec<_>>();
    observe_production_prefix(&mut authority, &hashes)
}

fn production_source_pressure(source: SourceRole, pressure: EffectPressure) -> FrontierObservation {
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let transaction = transaction_for_role(0, CellRole::None, None);
    let payload = resolved_payload_with_facts(
        &transaction,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(ranked_fee(0)),
    );
    let hash = match source {
        SourceRole::Trusted => {
            verify_proposal_with_payload(&mut authority, transaction, 0, payload)
        }
        SourceRole::Remote => {
            verify_remote_transaction_with_payload(&mut authority, transaction, 1_000, payload)
        }
    };
    fill_effect_region(&mut authority, EffectPolicy::Remote, 210);
    if pressure == EffectPressure::OrdinaryFull {
        fill_effect_region(&mut authority, EffectPolicy::Trusted, 220);
    }
    observe_production_prefix(&mut authority, &[hash])
}

fn production_stale_observation() -> FrontierObservation {
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let transaction = transaction_for_role(0, CellRole::None, None);
    let hash = verify_proposal(&mut authority, transaction, 0, ranked_fee(0));
    let batch = independent_batch(&authority, &[hash]);
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([231; 32])));
    match authority.plan_settlement(&batch) {
        Err(PlanError::Stale(_)) => FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::Stale,
        },
        Ok(plan) => {
            drop(plan);
            panic!("a stale production Ready receipt was accepted")
        }
        Err(error) => panic!("a stale production Ready receipt returned {error:?}"),
    }
}

fn production_shared_header_observation(owner_count: usize) -> FrontierObservation {
    let header = Byte32::new([232; 32]);
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let hashes = (0..owner_count)
        .map(|index| {
            let transaction = TransactionBuilder::default()
                .version(
                    30_000u32
                        .checked_add(u32::try_from(index).expect("finite header index fits u32"))
                        .expect("finite header version is bounded"),
                )
                .header_dep(header.clone())
                .build();
            verify_proposal(&mut authority, transaction, index, ranked_fee(index))
        })
        .collect::<Vec<_>>();
    observe_production_prefix(&mut authority, &hashes)
}

fn fill_effect_region(authority: &mut TxPoolAuthority, policy: EffectPolicy, marker: u8) {
    for index in 0u8..32 {
        let publication = authority
            .effect_publication_for_foundation(
                policy,
                vec![CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                    tx_hash: RawTxHash(Byte32::new([marker.wrapping_add(index); 32])),
                    requesting_peer: None,
                })],
            )
            .expect("one finite effect envelope fits its immutable batch bound");
        match authority.plan_effect_publication_for_foundation(&publication) {
            Ok(plan) => {
                apply_plan(plan);
            }
            Err(PlanError::Backpressure(Backpressure::EffectCapacity)) => return,
            Err(error) => panic!("finite effect fill returned {error:?}"),
        }
    }
    panic!("finite effect region did not reach its configured bound")
}

fn production_ready_observation(
    transactions: &[TransactionView],
    fees: &[u64],
) -> (Vec<usize>, Vec<ClaimTransactionCost>) {
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let hashes = transactions
        .iter()
        .cloned()
        .zip(fees.iter().copied())
        .enumerate()
        .map(|(index, (transaction, fee))| verify_proposal(&mut authority, transaction, index, fee))
        .collect::<Vec<_>>();
    let costs = hashes
        .iter()
        .map(|hash| {
            let owner = authority
                .entries_for_reference()
                .get(hash)
                .expect("every finite hash retains one Ready owner");
            let OwnedTx::PreAccepted(entry) = owner else {
                panic!("the finite order fixture has not entered Accepted membership");
            };
            let PreAcceptedPhase::Ready(verified) = &entry.phase else {
                panic!("the finite order fixture has sealed verification evidence");
            };
            production_cost_receipt(&entry.record.tx, verified.metrics())
                .expect("the production cost coordinates form the claim quotient")
        })
        .collect::<Vec<_>>();
    let order = authority
        .ready_for_reference()
        .into_iter()
        .map(|(hash, _)| {
            hashes
                .iter()
                .position(|candidate| candidate == &hash)
                .expect("every production Ready owner belongs to the finite fixture")
        })
        .collect();
    (order, costs)
}

fn production_cost_receipt(
    transaction: &TransactionView,
    metrics: &CandidateMetrics,
) -> Option<ClaimTransactionCost> {
    let payload_bytes = u32::try_from(transaction.data().total_size()).ok()?;
    let cost = ClaimTransactionCost::new(payload_bytes, metrics.fee.as_u64(), metrics.cost.cycles)?;
    (usize::try_from(cost.serialized_bytes()).ok()? == metrics.cost.serialized_bytes
        && metrics.cost.serialized_bytes == transaction.data().serialized_size_in_block())
    .then_some(cost)
}

fn ready_order_transaction(index: usize, payload_bytes: usize) -> TransactionView {
    let marker = u8::try_from(index + 1).expect("the finite order index fits u8");
    TransactionBuilder::default()
        .version(
            40_000u32
                .checked_add(u32::try_from(index).expect("finite order index fits u32"))
                .expect("finite order version is bounded"),
        )
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![marker; payload_bytes]).pack())
        .build()
}

fn transactions_for_roles(roles: &[CellRole], marker: u8) -> Vec<TransactionView> {
    let output_index = roles.iter().position(|role| *role == CellRole::Output);
    let mut transactions = vec![None; roles.len()];
    let shared = if let Some(index) = output_index {
        let transaction = transaction_for_role(index, CellRole::Output, None);
        let output = OutPoint::new(transaction.hash(), 0);
        transactions[index] = Some(transaction);
        output
    } else {
        OutPoint::new(Byte32::new([marker; 32]), 0)
    };
    for (index, role) in roles.iter().copied().enumerate() {
        if transactions[index].is_none() {
            transactions[index] = Some(transaction_for_role(index, role, Some(&shared)));
        }
    }
    transactions
        .into_iter()
        .map(|transaction| transaction.expect("every finite role has one transaction"))
        .collect()
}

fn transaction_for_role(
    index: usize,
    role: CellRole,
    shared: Option<&OutPoint>,
) -> TransactionView {
    let mut builder = TransactionBuilder::default().version(
        20_000u32
            .checked_add(u32::try_from(index).expect("finite role index fits u32"))
            .expect("finite role version is bounded"),
    );
    match role {
        CellRole::None => {}
        CellRole::Input => {
            builder = builder.input(CellInput::new(
                shared.expect("an input role has a shared cell").clone(),
                0,
            ));
        }
        CellRole::Read => {
            builder = builder.cell_dep(
                CellDep::new_builder()
                    .out_point(shared.expect("a read role has a shared cell").clone())
                    .build(),
            );
        }
        CellRole::Output => {
            builder = builder
                .output(CellOutput::default())
                .output_data(Bytes::new().pack());
        }
    }
    builder.build()
}

fn verify_proposal(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    index: usize,
    fee: u64,
) -> RawTxHash {
    let payload = resolved_payload_with_facts(
        &transaction,
        Vec::new(),
        transaction.input_pts_iter().collect(),
        Capacity::shannons(fee),
    );
    verify_proposal_with_payload(authority, transaction, index, payload)
}

fn verify_proposal_with_payload(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    index: usize,
    payload: crate::authority::state::test_support::FoundationResolution,
) -> RawTxHash {
    let hash = verify_remote_transaction_with_payload(
        authority,
        transaction.clone(),
        1_000 + index,
        payload,
    );
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction)
                    .expect("the finite proposal role is structurally valid"),
            )
            .expect("proposal promotion preserves the verified Ready owner"),
    );
    hash
}

fn observe_production_prefix(
    authority: &mut TxPoolAuthority,
    hashes: &[RawTxHash],
) -> FrontierObservation {
    let ordered = authority
        .ready_for_reference()
        .into_iter()
        .map(|(hash, _)| hash)
        .filter(|hash| hashes.contains(hash))
        .collect::<Vec<_>>();
    assert_eq!(
        ordered.len(),
        hashes.len(),
        "every finite candidate must occupy the production Ready frontier"
    );
    let mut prefix_len = 0usize;
    for requested in 1..=ordered.len() {
        let batch = independent_batch(authority, &ordered[..requested]);
        match authority.plan_settlement(&batch) {
            Err(PlanError::Backpressure(Backpressure::EffectCapacity)) => {
                return FrontierObservation {
                    prefix_len,
                    terminal: FrontierTerminal::Coupled,
                };
            }
            Err(PlanError::Stale(_)) => {
                return FrontierObservation {
                    prefix_len,
                    terminal: FrontierTerminal::Stale,
                };
            }
            Err(error) => panic!("a finite valid role case returned {error:?}"),
            Ok(SettlementPlan::IndependentRun(plan)) => {
                let selected = plan
                    .independent_order_for_foundation()
                    .expect("the independent plan exposes its sealed test order")
                    .len();
                drop(plan);
                if selected == requested {
                    prefix_len = requested;
                    continue;
                }
                return FrontierObservation {
                    prefix_len: selected,
                    terminal: FrontierTerminal::Coupled,
                };
            }
            Ok(SettlementPlan::CoupledComponent(plan)) => {
                drop(plan);
                return FrontierObservation {
                    prefix_len,
                    terminal: FrontierTerminal::Coupled,
                };
            }
        }
    }
    FrontierObservation {
        prefix_len,
        terminal: FrontierTerminal::Complete,
    }
}

fn assert_duplicate_output_premise(roles: &[CellRole]) {
    let outputs = roles
        .iter()
        .enumerate()
        .filter(|(_, role)| **role == CellRole::Output)
        .map(|(index, _)| {
            let transaction = transaction_for_role(index, CellRole::Output, None);
            OutPoint::new(transaction.hash(), 0)
        })
        .collect::<Vec<_>>();
    assert!(
        outputs
            .array_windows::<2>()
            .all(|[left, right]| left != right),
        "distinct raw transaction identities cannot produce one exact outpoint"
    );
}

fn ranked_fee(index: usize) -> u64 {
    RANKED_FEES
        .get(index)
        .copied()
        .expect("the finite refinement rank fits the production batch")
}

fn production_eviction_fixture() -> TxPoolAuthority {
    let mut authority = TxPoolAuthority::for_foundation(eviction_refinement_limits());
    let parent_input = OutPoint::new(Byte32::new([210; 32]), 0);
    let parent = TransactionBuilder::default()
        .version(20_000u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![0; 32]).pack())
        .build();
    let parent_payload = resolved_payload_with_facts(
        &parent,
        Vec::new(),
        vec![parent_input],
        Capacity::shannons(100),
    );
    accept_remote_transaction_with_payload_and_cycles(
        &mut authority,
        parent.clone(),
        2_000,
        AcceptedStatus::Pending,
        parent_payload,
        2_000_000,
    );

    let child = TransactionBuilder::default()
        .version(20_001u32)
        .input(CellInput::new(OutPoint::new(parent.hash(), 0), 0))
        .build();
    let child_payload =
        resolved_payload_with_facts(&child, Vec::new(), Vec::new(), Capacity::shannons(10_000));
    accept_remote_transaction_with_payload(
        &mut authority,
        child,
        2_001,
        AcceptedStatus::Pending,
        child_payload,
    );

    for (offset, status, fee, cycles) in [
        (0u8, AcceptedStatus::Gap, 1u64, 0u64),
        (1, AcceptedStatus::Proposed, 596, 3_500_000),
        (2, AcceptedStatus::Pending, 7, 0),
        (3, AcceptedStatus::Pending, 7, 0),
    ] {
        let chain_input = OutPoint::new(Byte32::new([220 + offset; 32]), 0);
        let transaction = TransactionBuilder::default()
            .version(20_010 + u32::from(offset))
            .input(CellInput::new(chain_input.clone(), 0))
            .build();
        let payload = resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            vec![chain_input],
            Capacity::shannons(fee),
        );
        accept_remote_transaction_with_payload_and_cycles(
            &mut authority,
            transaction,
            2_010 + usize::from(offset),
            status,
            payload,
            cycles,
        );
    }
    authority
}

fn refinement_status(status: AcceptedStatus) -> EvictionRefinementStatus {
    match status {
        AcceptedStatus::Pending => EvictionRefinementStatus::Pending,
        AcceptedStatus::Gap => EvictionRefinementStatus::Gap,
        AcceptedStatus::Proposed => EvictionRefinementStatus::Proposed,
    }
}

fn production_eviction_status_receipt(
    receipt: &ProposalContextReceipt,
) -> EvictionRefinementStatus {
    eviction_status_witness(refinement_status(receipt.status()))
}

fn refinement_identity(hash: &RawTxHash) -> [u8; 32] {
    let mut identity = [0; 32];
    identity.copy_from_slice(hash.0.as_slice());
    identity
}

fn eviction_refinement_limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(16, 256 * 1024, 256, 16),
        ResourceVector::new(16, 256 * 1024, 256, 16),
        ResourceVector::new(16, 256 * 1024, 256, 16),
        AcceptedResources::new(16, 256 * 1024, 256 * 1024, 20_000_000),
        ComputeLimits::new(16 * 1024, 16 * 1024, 256),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(8, 128 * 1024, 128, 0))
    })
    .expect("the finite eviction-refinement fixture has valid resource partitions")
}

fn refinement_limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(16, 256 * 1024, 256, 16),
        ResourceVector::new(16, 256 * 1024, 256, 16),
        ResourceVector::new(16, 256 * 1024, 256, 16),
        AcceptedResources::new(16, 256 * 1024, 256 * 1024, 256),
        ComputeLimits::new(16 * 1024, 16 * 1024, 256),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(8, 128 * 1024, 128, 0))
    })
    .expect("the finite refinement universe has a valid bounded resource partition")
}
