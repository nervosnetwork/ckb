//! Pointwise refinement of the retained-compute scheduler quotient.
//!
//! The reference side exposes only a finite input algebra and normalized
//! observation. This adapter independently extracts real authority owners,
//! calls real checkout plans in stable worker-role order, and compares the
//! resulting assignments and committed fairness cursors.

use super::claim_relations::{
    SchedulerRefinementAssignment, SchedulerRefinementCapability, SchedulerRefinementCursors,
    SchedulerRefinementEntry, SchedulerRefinementObservation, SchedulerRefinementOwner,
    SchedulerRefinementPermit, SchedulerRefinementSource, SchedulerRefinementStage,
    SchedulerRefinementVerifyClass, SchedulerRefinementVerifyOrder, SchedulerRefinementWorker,
    SchedulerRefinementWorkerRole, scheduler_wave_observation,
};
use super::foundation::{
    apply_plan, limits, owner_version, resolved_payload_with_facts, take_resolve_work, tx,
};
use crate::authority::{
    exchange::{ComputeVerifierSlot, ComputeWorkerSlot, ComputeWorkerSlotId},
    plan::TxPoolAuthority,
    scheduler::{VerifyOrder, WorkOwner},
    state::{
        OwnedTx, PreAcceptedPhase, PreAcceptedSource, QueuedWork, RawTxHash, TxIdentity,
        ValidatedAdmission, VerifyCapability, VerifyCycleClass, WorkPermit,
    },
    work::CheckedOutWork,
};
use ckb_network::PeerIndex;
use ckb_types::core::Capacity;
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub(super) struct Symbol {
    pub(super) transaction: u8,
    pub(super) owner: SchedulerRefinementOwner,
}

pub(super) fn refinement_owner(owner: WorkOwner) -> SchedulerRefinementOwner {
    match owner {
        WorkOwner::Remote(peer) => SchedulerRefinementOwner::Remote(
            u8::try_from(peer.value()).expect("the finite fixture peer fits u8"),
        ),
        WorkOwner::Trusted => SchedulerRefinementOwner::Trusted,
    }
}

fn refinement_cursors(authority: &TxPoolAuthority) -> SchedulerRefinementCursors {
    let (resolve, verify) = authority.scheduler_cursors_for_refinement();
    SchedulerRefinementCursors {
        resolve: resolve.map(refinement_owner),
        verify: verify.map(refinement_owner),
    }
}

fn refinement_source(source: PreAcceptedSource) -> SchedulerRefinementSource {
    match source {
        PreAcceptedSource::Remote(remote) => SchedulerRefinementSource::Remote(
            u8::try_from(remote.residency.peer.value()).expect("the finite fixture peer fits u8"),
        ),
        PreAcceptedSource::Proposal { .. } => SchedulerRefinementSource::Proposal,
        PreAcceptedSource::Recovery(_) => SchedulerRefinementSource::Recovery,
    }
}

fn refinement_class(class: VerifyCycleClass) -> SchedulerRefinementVerifyClass {
    match class {
        VerifyCycleClass::Small => SchedulerRefinementVerifyClass::Small,
        VerifyCycleClass::Large => SchedulerRefinementVerifyClass::Large,
    }
}

fn refinement_permit(permit: WorkPermit) -> SchedulerRefinementPermit {
    let capability = |capability| match capability {
        VerifyCapability::SmallCycleOnly => SchedulerRefinementCapability::SmallOnly,
        VerifyCapability::Any => SchedulerRefinementCapability::Any,
    };
    match permit {
        WorkPermit::ResolveOnly => SchedulerRefinementPermit::ResolveOnly,
        WorkPermit::ResolveThenVerify(value) => {
            SchedulerRefinementPermit::ResolveThenVerify(capability(value))
        }
        WorkPermit::VerifyOnly(value) => SchedulerRefinementPermit::VerifyOnly(capability(value)),
    }
}

fn refinement_order(order: VerifyOrder) -> SchedulerRefinementVerifyOrder {
    match order {
        VerifyOrder::Arrival => SchedulerRefinementVerifyOrder::Arrival,
        VerifyOrder::FeeRate => SchedulerRefinementVerifyOrder::FeeRate,
    }
}

/// Normalize the complete production scheduler projection. Owner states that
/// do not occupy a scheduler slot deliberately map to `None`; Ready remains a
/// set member even though the compute-worker quotient cannot select it.
pub(super) fn refinement_projection_entry(
    owner: &OwnedTx,
    symbol: Symbol,
) -> Option<SchedulerRefinementEntry> {
    let OwnedTx::PreAccepted(entry) = owner else {
        return None;
    };
    let (stage, fee, bytes) = match &entry.phase {
        PreAcceptedPhase::Queued(QueuedWork::Resolve) => (
            SchedulerRefinementStage::Resolve,
            0,
            u32::try_from(entry.record.tx.data().total_size())
                .expect("the finite fixture transaction size fits u32"),
        ),
        PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)) => (
            SchedulerRefinementStage::Verify(refinement_class(resolved.verify_class())),
            resolved.payload().fee().as_u64(),
            u32::try_from(resolved.payload().serialized_bytes())
                .expect("the finite fixture serialized size fits u32"),
        ),
        PreAcceptedPhase::Ready(verified) => (
            SchedulerRefinementStage::Ready,
            verified.metrics().fee.as_u64(),
            u32::try_from(verified.payload().serialized_bytes())
                .expect("the finite Ready size fits u32"),
        ),
        PreAcceptedPhase::Computing(_) | PreAcceptedPhase::Waiting(_) => return None,
    };
    Some(SchedulerRefinementEntry {
        transaction: symbol.transaction,
        version: u16::try_from(entry.record.version.0)
            .expect("the finite fixture version fits u16"),
        arrival: u16::try_from(entry.record.arrival.0)
            .expect("the finite fixture arrival fits u16"),
        source: refinement_source(entry.source),
        stage,
        fee,
        bytes,
    })
}

fn refinement_entries(
    authority: &TxPoolAuthority,
    symbols: &HashMap<RawTxHash, Symbol>,
) -> Vec<SchedulerRefinementEntry> {
    let mut entries = symbols
        .iter()
        .map(|(hash, symbol)| {
            let owner = authority
                .entries_for_reference()
                .get(hash)
                .expect("the symbolic transaction remains owned");
            refinement_projection_entry(&owner, *symbol)
                .expect("the scheduler wave fixture contains only runnable owners")
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by_key(|entry| entry.transaction);
    entries
}

fn apply_checkout(
    authority: &mut TxPoolAuthority,
    symbols: &HashMap<RawTxHash, Symbol>,
    slot: u8,
    permit: WorkPermit,
) -> Option<SchedulerRefinementAssignment> {
    let checkout = authority
        .plan_checkout_next(permit)
        .expect("the production scheduler cut is valid")?
        .apply()
        .into_work();
    let transaction = match &checkout {
        CheckedOutWork::Resolve(work) => work.transaction(),
        CheckedOutWork::ContinuousResolve(work) => work.transaction(),
        CheckedOutWork::Verify(work) => work.transaction(),
    };
    let hash = TxIdentity::from_transaction(transaction).raw;
    let symbol = symbols
        .get(&hash)
        .expect("every selected production transaction has a symbolic identity");
    Some(SchedulerRefinementAssignment {
        slot,
        transaction: symbol.transaction,
        owner: symbol.owner,
        permit: refinement_permit(permit),
    })
}

fn production_wave(
    authority: &mut TxPoolAuthority,
    symbols: &HashMap<RawTxHash, Symbol>,
    workers: &[(u8, WorkPermit)],
) -> SchedulerRefinementObservation {
    let mut assignments = Vec::new();
    let mut idle_slots = Vec::new();
    for (slot, permit) in workers.iter().copied() {
        match apply_checkout(authority, symbols, slot, permit) {
            Some(assignment) => assignments.push(assignment),
            None => idle_slots.push(slot),
        }
    }
    SchedulerRefinementObservation {
        assignments,
        idle_slots,
        cursors: refinement_cursors(authority),
    }
}

fn refinement_slot(slot: ComputeWorkerSlot) -> u8 {
    match slot.id() {
        ComputeWorkerSlotId::OrderedResolve => 0,
        ComputeWorkerSlotId::Verifier(worker_id) => u8::try_from(
            worker_id
                .checked_add(1)
                .expect("the finite worker id has a successor"),
        )
        .expect("the finite worker id fits u8"),
    }
}

fn virtual_production_wave(
    authority: &TxPoolAuthority,
    symbols: &HashMap<RawTxHash, Symbol>,
    slots: &[ComputeWorkerSlot],
) -> SchedulerRefinementObservation {
    let wave = authority
        .scheduler_worker_wave_for_refinement(slots)
        .expect("the production scheduler wave plans");
    let assignments = wave
        .assignments
        .into_iter()
        .map(|(slot, permit, hash, _)| {
            let symbol = symbols
                .get(&hash)
                .expect("every virtual assignment has a symbolic identity");
            SchedulerRefinementAssignment {
                slot: refinement_slot(slot),
                transaction: symbol.transaction,
                owner: symbol.owner,
                permit: refinement_permit(permit),
            }
        })
        .collect();
    SchedulerRefinementObservation {
        assignments,
        idle_slots: wave.idle.into_iter().map(refinement_slot).collect(),
        cursors: SchedulerRefinementCursors {
            resolve: wave.cursors.0.map(refinement_owner),
            verify: wave.cursors.1.map(refinement_owner),
        },
    }
}

fn admit(
    authority: &mut TxPoolAuthority,
    admission: ValidatedAdmission,
    transaction: u8,
    owner: SchedulerRefinementOwner,
    symbols: &mut HashMap<RawTxHash, Symbol>,
) -> RawTxHash {
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("the refinement fixture admission plans"),
    );
    symbols.insert(hash.clone(), Symbol { transaction, owner });
    hash
}

fn queue_remote_verify(
    authority: &mut TxPoolAuthority,
    transaction: ckb_types::core::TransactionView,
    peer: u8,
    fee: u64,
    class: VerifyCycleClass,
    symbol: u8,
    symbols: &mut HashMap<RawTxHash, Symbol>,
) -> RawTxHash {
    let admission =
        ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(usize::from(peer)))
            .expect("the remote refinement admission is valid");
    let hash = admit(
        authority,
        admission,
        symbol,
        SchedulerRefinementOwner::Remote(peer),
        symbols,
    );
    let (_, resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &hash,
                owner_version(authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("the exact resolve fixture plans")
            .apply(),
    );
    let payload = resolved_payload_with_facts(
        &transaction,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(fee),
    );
    let settlement = match class {
        VerifyCycleClass::Small => resolve
            .yield_verify(payload)
            .expect("the small payload matches its resolve capability"),
        VerifyCycleClass::Large => resolve
            .yield_verify_as(payload, VerifyCycleClass::Large)
            .expect("the large payload matches its resolve capability"),
    };
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the queued Verify settlement plans"),
    );
    hash
}

#[test]
fn uak_multi_owner_resolve_wave_refines_the_scheduler_quotient_pointwise() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let mut symbols = HashMap::new();
    let trusted = ValidatedAdmission::proposal(tx(1_001)).expect("trusted fixture is valid");
    admit(
        &mut authority,
        trusted,
        1,
        SchedulerRefinementOwner::Trusted,
        &mut symbols,
    );
    for (nonce, peer, symbol) in [(1_002, 1, 2), (1_003, 1, 3), (1_004, 2, 4)] {
        let admission = ValidatedAdmission::remote(tx(nonce), PeerIndex::from(usize::from(peer)))
            .expect("remote fixture is valid");
        admit(
            &mut authority,
            admission,
            symbol,
            SchedulerRefinementOwner::Remote(peer),
            &mut symbols,
        );
    }

    let workers = [
        SchedulerRefinementWorker {
            slot: 8,
            role: SchedulerRefinementWorkerRole::VerifyAny,
        },
        SchedulerRefinementWorker {
            slot: 0,
            role: SchedulerRefinementWorkerRole::OrderedResolve,
        },
        SchedulerRefinementWorker {
            slot: 5,
            role: SchedulerRefinementWorkerRole::VerifySmall,
        },
    ];
    let expected = scheduler_wave_observation(
        &refinement_entries(&authority, &symbols),
        &workers,
        refinement_cursors(&authority),
        SchedulerRefinementVerifyOrder::Arrival,
    )
    .expect("the finite scheduler input is valid");
    let before = authority.normalized_snapshot();
    let virtual_wave = virtual_production_wave(
        &authority,
        &symbols,
        &[
            ComputeVerifierSlot::new(7, VerifyCapability::Any).into(),
            ComputeWorkerSlot::ordered_resolve(),
            ComputeVerifierSlot::new(4, VerifyCapability::SmallCycleOnly).into(),
        ],
    );
    assert_eq!(virtual_wave, expected);
    assert_eq!(authority.normalized_snapshot(), before);
    let actual = production_wave(
        &mut authority,
        &symbols,
        &[
            (0, WorkPermit::ResolveOnly),
            (
                5,
                WorkPermit::ResolveThenVerify(VerifyCapability::SmallCycleOnly),
            ),
            (8, WorkPermit::ResolveThenVerify(VerifyCapability::Any)),
        ],
    );
    assert_eq!(actual, expected);
}

#[test]
fn uak_verify_capability_wave_refines_the_scheduler_quotient_pointwise() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let mut symbols = HashMap::new();
    queue_remote_verify(
        &mut authority,
        tx(1_011),
        1,
        1,
        VerifyCycleClass::Small,
        1,
        &mut symbols,
    );
    queue_remote_verify(
        &mut authority,
        tx(1_012),
        2,
        1_000,
        VerifyCycleClass::Large,
        2,
        &mut symbols,
    );
    let workers = [
        SchedulerRefinementWorker {
            slot: 2,
            role: SchedulerRefinementWorkerRole::VerifyAny,
        },
        SchedulerRefinementWorker {
            slot: 1,
            role: SchedulerRefinementWorkerRole::VerifySmall,
        },
    ];
    let expected = scheduler_wave_observation(
        &refinement_entries(&authority, &symbols),
        &workers,
        refinement_cursors(&authority),
        SchedulerRefinementVerifyOrder::Arrival,
    )
    .expect("the finite scheduler input is valid");
    let before = authority.normalized_snapshot();
    let virtual_wave = virtual_production_wave(
        &authority,
        &symbols,
        &[
            ComputeVerifierSlot::new(1, VerifyCapability::Any).into(),
            ComputeVerifierSlot::new(0, VerifyCapability::SmallCycleOnly).into(),
        ],
    );
    assert_eq!(virtual_wave, expected);
    assert_eq!(authority.normalized_snapshot(), before);
    let actual = production_wave(
        &mut authority,
        &symbols,
        &[
            (1, WorkPermit::VerifyOnly(VerifyCapability::SmallCycleOnly)),
            (2, WorkPermit::VerifyOnly(VerifyCapability::Any)),
        ],
    );
    assert_eq!(actual, expected);
}

#[test]
fn uak_verify_order_modes_refine_the_scheduler_quotient_pointwise() {
    for order in [VerifyOrder::Arrival, VerifyOrder::FeeRate] {
        let mut authority = TxPoolAuthority::for_foundation_with_order(limits(), order);
        let mut symbols = HashMap::new();
        queue_remote_verify(
            &mut authority,
            tx(1_021),
            1,
            1,
            VerifyCycleClass::Small,
            1,
            &mut symbols,
        );
        queue_remote_verify(
            &mut authority,
            tx(1_022),
            1,
            1_000,
            VerifyCycleClass::Small,
            2,
            &mut symbols,
        );
        let workers = [SchedulerRefinementWorker {
            slot: 1,
            role: SchedulerRefinementWorkerRole::VerifyAny,
        }];
        let expected = scheduler_wave_observation(
            &refinement_entries(&authority, &symbols),
            &workers,
            refinement_cursors(&authority),
            refinement_order(order),
        )
        .expect("the finite scheduler input is valid");
        let before = authority.normalized_snapshot();
        let virtual_wave = virtual_production_wave(
            &authority,
            &symbols,
            &[ComputeVerifierSlot::new(0, VerifyCapability::Any).into()],
        );
        assert_eq!(virtual_wave, expected, "virtual verify order {order:?}");
        assert_eq!(authority.normalized_snapshot(), before);
        let actual = production_wave(
            &mut authority,
            &symbols,
            &[(1, WorkPermit::VerifyOnly(VerifyCapability::Any))],
        );
        assert_eq!(actual, expected, "verify order {order:?}");
    }
}
