//! Pointwise refinement of the bounded membership frontier.
//!
//! The shared module supplies only symbolic cases and model observations.
//! This file independently constructs production transactions, receipts and
//! authority state, then calls the production settlement planner.

use super::foundation::{
    accept_remote_transaction_with_payload, apply_plan, independent_batch,
    resolved_payload_with_facts, verify_remote_transaction_with_payload,
};
use crate::{
    authority::{
        effect::{CommittedAcceptance, CommittedEffect, EffectPolicy},
        plan::{Backpressure, PlanError, SettlementPlan, TxPoolAuthority},
        resources::{AcceptedResources, ComputeLimits, ResourceLimits, ResourceVector},
        scheduler::MAX_READY_BATCH,
        state::{
            AcceptedStatus, ChainRevision, ChainViewId, RawTxHash, ResolvedPayload,
            ValidatedAdmission,
        },
    },
    mathematical_model::{
        CellRole, EffectPressure, EvidenceOriginRole, FrontierObservation, FrontierTerminal,
        REFINEMENT_MAX_READY, ReadyOrderInput, SourceRole, accepted_capacity_observation,
        accepted_role_observation, candidate_graph_observation, candidate_role_observation,
        evidence_origin_observation, positioned_role_observation, ready_order_observation,
        shared_header_observation, source_observation, source_pressure_observation,
        stale_observation,
    },
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
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
fn uak_candidate_role_product_refines_the_executable_model_pointwise() {
    for left in CellRole::ALL {
        for right in CellRole::ALL {
            let expected = candidate_role_observation(left, right);
            let actual = production_candidate_roles(&[left, right]);
            assert_eq!(actual, expected, "candidate roles {left:?}/{right:?}");
        }
    }
}

#[test]
fn uak_candidate_accepted_role_product_refines_the_executable_model_pointwise() {
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
fn uak_every_four_owner_coupling_graph_refines_the_model_prefix() {
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
fn uak_every_single_coupled_edge_position_refines_the_model_prefix() {
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
fn uak_source_control_classes_refine_the_model_prefix() {
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
fn uak_stale_ready_evidence_refines_the_model_terminal() {
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
fn uak_ready_economic_order_refines_when_fee_and_rate_disagree() {
    let transactions = [
        ready_order_transaction(0, 2_048),
        ready_order_transaction(1, 0),
        ready_order_transaction(2, 512),
    ];
    let fees = [3_000, 1_000, 2_000];
    let inputs = transactions
        .iter()
        .zip(fees)
        .map(|(transaction, fee)| ReadyOrderInput {
            fee,
            serialized_bytes: u32::try_from(transaction.data().total_size())
                .expect("the finite transaction size fits u32"),
        })
        .collect::<Vec<_>>();
    let expected = ready_order_observation(&inputs);
    assert_ne!(
        expected,
        vec![0, 2, 1],
        "the fixture must make absolute-fee order differ from fee-rate order"
    );
    assert_eq!(production_ready_order(&transactions, &fees), expected);
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

fn production_ready_order(transactions: &[TransactionView], fees: &[u64]) -> Vec<usize> {
    let mut authority = TxPoolAuthority::for_foundation(refinement_limits());
    let hashes = transactions
        .iter()
        .cloned()
        .zip(fees.iter().copied())
        .enumerate()
        .map(|(index, (transaction, fee))| verify_proposal(&mut authority, transaction, index, fee))
        .collect::<Vec<_>>();
    authority
        .ready_for_reference()
        .into_iter()
        .map(|(hash, _)| {
            hashes
                .iter()
                .position(|candidate| candidate == &hash)
                .expect("every production Ready owner belongs to the finite fixture")
        })
        .collect()
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
        outputs.windows(2).all(|pair| pair[0] != pair[1]),
        "distinct raw transaction identities cannot produce one exact outpoint"
    );
}

fn ranked_fee(index: usize) -> u64 {
    RANKED_FEES
        .get(index)
        .copied()
        .expect("the finite refinement rank fits the production batch")
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
