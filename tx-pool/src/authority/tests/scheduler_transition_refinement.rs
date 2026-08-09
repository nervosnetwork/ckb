//! Finite production refinement for scheduler set and owner-ring transitions.
//!
//! Every production fixture is created through admission, checkout and
//! settlement constructors. The test then drives the real `FairFrontier`
//! Plan/Apply API and compares only normalized scheduler observations with the
//! independent reference relation.

use super::{
    foundation::{
        apply_plan, limits, owner_version, resolved_payload_with_facts, take_resolve_work, tx,
    },
    scheduler_refinement::{Symbol, refinement_owner, refinement_projection_entry},
};
use crate::{
    authority::{
        plan::TxPoolAuthority,
        scheduler::{
            FairFrontier, VerifyOrder, WorkOwner,
            test_support::{SchedulerSetMemberObservation, SchedulerSetStageObservation},
        },
        state::{
            EntryVersion, OwnedTx, RawTxHash, ValidatedAdmission, VerifyCapability,
            VerifyCycleClass, WorkPermit,
        },
        work::CheckedOutWork,
    },
    mathematical_model::{
        SchedulerOwnerPopulation, SchedulerOwnerRing, SchedulerProjectionChange,
        SchedulerRefinementCursors, SchedulerRefinementOwner, SchedulerRefinementSource,
        SchedulerRefinementStage, SchedulerRefinementVerifyClass, SchedulerRefinementVerifyOrder,
        SchedulerSetProjection,
    },
};
use ckb_network::PeerIndex;
use ckb_types::core::Capacity;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone)]
struct SchedulerVariants {
    symbol: Symbol,
    resolve: OwnedTx,
    computing: OwnedTx,
    verify: OwnedTx,
    ready: OwnedTx,
}

#[derive(Clone, Copy, Debug)]
enum ProjectionState {
    Empty,
    Resolve,
    Computing,
    Verify,
    Ready,
}

const PROJECTION_STATES: [ProjectionState; 5] = [
    ProjectionState::Empty,
    ProjectionState::Resolve,
    ProjectionState::Computing,
    ProjectionState::Verify,
    ProjectionState::Ready,
];

impl SchedulerVariants {
    fn owner(&self, state: ProjectionState) -> Option<&OwnedTx> {
        match state {
            ProjectionState::Empty => None,
            ProjectionState::Resolve => Some(&self.resolve),
            ProjectionState::Computing => Some(&self.computing),
            ProjectionState::Verify => Some(&self.verify),
            ProjectionState::Ready => Some(&self.ready),
        }
    }
}

fn admission(
    transaction: ckb_types::core::TransactionView,
    source: SchedulerRefinementSource,
) -> ValidatedAdmission {
    match source {
        SchedulerRefinementSource::Remote(peer) => {
            ValidatedAdmission::remote(transaction, PeerIndex::from(usize::from(peer)))
                .expect("the finite remote admission is valid")
        }
        SchedulerRefinementSource::Proposal => {
            ValidatedAdmission::proposal(transaction).expect("the proposal admission is valid")
        }
        SchedulerRefinementSource::Recovery => {
            panic!("Recovery is not an ingress constructor in the finite producer bank")
        }
    }
}

fn build_variants(
    nonce: u64,
    transaction: u8,
    source: SchedulerRefinementSource,
    owner: SchedulerRefinementOwner,
    class: VerifyCycleClass,
) -> SchedulerVariants {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction_view = tx(nonce);
    let admission = admission(transaction_view.clone(), source);
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("the producer-bank admission plans"),
    );
    let resolve = authority
        .entries_for_reference()
        .get(&hash)
        .expect("the Resolve owner exists")
        .clone();

    let (_, resolve_work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &hash,
                owner_version(&authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("the exact Resolve checkout plans")
            .apply(),
    );
    let computing = authority
        .entries_for_reference()
        .get(&hash)
        .expect("checkout publishes Computing")
        .clone();
    let payload = resolved_payload_with_facts(
        &transaction_view,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(u64::from(transaction) + 1),
    );
    let settlement = resolve_work
        .yield_verify_as(payload, class)
        .expect("the finite resolution receipt is valid");
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("Resolve settlement publishes queued Verify"),
    );
    let verify = authority
        .entries_for_reference()
        .get(&hash)
        .expect("the Verify owner exists")
        .clone();

    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("the exact Verify checkout plans")
        .apply()
        .into_work();
    let CheckedOutWork::Verify(verify_work) = checkout else {
        panic!("the exact queued Verify owner yields Verify work");
    };
    apply_plan(
        authority
            .apply_settlement(verify_work.verified(0))
            .expect("verification settlement publishes Ready"),
    );
    let ready = authority
        .entries_for_reference()
        .get(&hash)
        .expect("the Ready owner exists")
        .clone();
    SchedulerVariants {
        symbol: Symbol { transaction, owner },
        resolve,
        computing,
        verify,
        ready,
    }
}

fn projection_state(index: usize) -> ProjectionState {
    PROJECTION_STATES[index]
}

fn state_vectors() -> Vec<[ProjectionState; 3]> {
    let mut states = Vec::with_capacity(PROJECTION_STATES.len().pow(3));
    for encoded in 0..PROJECTION_STATES.len().pow(3) {
        let mut value = encoded;
        let first = projection_state(value % PROJECTION_STATES.len());
        value /= PROJECTION_STATES.len();
        let second = projection_state(value % PROJECTION_STATES.len());
        value /= PROJECTION_STATES.len();
        let third = projection_state(value % PROJECTION_STATES.len());
        states.push([first, second, third]);
    }
    states
}

fn model_entry(
    variants: &SchedulerVariants,
    state: ProjectionState,
) -> Option<crate::mathematical_model::SchedulerRefinementEntry> {
    variants
        .owner(state)
        .and_then(|owner| refinement_projection_entry(owner, variants.symbol))
}

fn frontier_from_states(
    order: VerifyOrder,
    variants: &[SchedulerVariants; 3],
    states: [ProjectionState; 3],
) -> FairFrontier {
    let mut frontier = FairFrontier::new(order);
    let plan = frontier
        .plan_batch(
            variants
                .iter()
                .zip(states)
                .map(|(variants, state)| (None, variants.owner(state))),
        )
        .expect("the initial real-producer scheduler set plans");
    frontier.apply_batch(plan);
    frontier
}

fn expected_owner_map(
    variants: &[SchedulerVariants; 3],
    states: [ProjectionState; 3],
) -> HashMap<RawTxHash, OwnedTx> {
    variants
        .iter()
        .zip(states)
        .filter_map(|(variants, state)| {
            variants
                .owner(state)
                .map(|owner| (owner.record().identity.raw.clone(), owner.clone()))
        })
        .collect()
}

fn expected_set_observation(
    projection: &SchedulerSetProjection,
    variants: &[SchedulerVariants; 3],
) -> BTreeSet<SchedulerSetMemberObservation> {
    projection
        .entries()
        .values()
        .map(|entry| {
            let variants = variants
                .iter()
                .find(|variants| variants.symbol.transaction == entry.transaction)
                .expect("every model transaction belongs to the producer bank");
            let owner = production_owner(variants.symbol.owner);
            let stage = match entry.stage {
                SchedulerRefinementStage::Resolve => SchedulerSetStageObservation::Resolve(owner),
                SchedulerRefinementStage::Verify(class) => {
                    let class = match class {
                        SchedulerRefinementVerifyClass::Small => VerifyCycleClass::Small,
                        SchedulerRefinementVerifyClass::Large => VerifyCycleClass::Large,
                    };
                    SchedulerSetStageObservation::Verify(owner, class)
                }
                SchedulerRefinementStage::Ready => SchedulerSetStageObservation::Ready,
            };
            SchedulerSetMemberObservation {
                hash: variants.resolve.record().identity.raw.clone(),
                version: EntryVersion(u128::from(entry.version)),
                stage,
            }
        })
        .collect()
}

#[test]
fn uak_scheduler_set_transition_refines_every_real_projection_state_and_order() {
    let variants = [
        build_variants(
            2_001,
            1,
            SchedulerRefinementSource::Proposal,
            SchedulerRefinementOwner::Trusted,
            VerifyCycleClass::Small,
        ),
        build_variants(
            2_002,
            2,
            SchedulerRefinementSource::Remote(1),
            SchedulerRefinementOwner::Remote(1),
            VerifyCycleClass::Small,
        ),
        build_variants(
            2_003,
            3,
            SchedulerRefinementSource::Remote(2),
            SchedulerRefinementOwner::Remote(2),
            VerifyCycleClass::Large,
        ),
    ];
    let states = state_vectors();
    for (production_order, model_order) in [
        (
            VerifyOrder::Arrival,
            SchedulerRefinementVerifyOrder::Arrival,
        ),
        (
            VerifyOrder::FeeRate,
            SchedulerRefinementVerifyOrder::FeeRate,
        ),
    ] {
        for initial in &states {
            let model = SchedulerSetProjection::new(
                variants
                    .iter()
                    .zip(*initial)
                    .filter_map(|(variants, state)| model_entry(variants, state)),
                model_order,
                SchedulerRefinementCursors::default(),
            )
            .expect("the real producer bank has unique nonzero scheduler entries");
            for after in &states {
                let changes = variants
                    .iter()
                    .enumerate()
                    .map(|(index, variants)| SchedulerProjectionChange {
                        transaction: variants.symbol.transaction,
                        expected: model_entry(variants, initial[index]),
                        after: model_entry(variants, after[index]),
                    })
                    .collect::<Vec<_>>();
                let expected = model
                    .plan_changes(&changes, SchedulerRefinementCursors::default())
                    .expect("each finite owner transition is identity coherent");

                let mut actual = frontier_from_states(production_order, &variants, *initial);
                let plan = actual
                    .plan_batch(variants.iter().enumerate().map(|(index, variants)| {
                        (variants.owner(initial[index]), variants.owner(after[index]))
                    }))
                    .expect("the production set transition accepts the same legal input");
                actual.apply_batch(plan);

                let owner_map = expected_owner_map(&variants, *after);
                assert!(actual.semantically_matches(&owner_map));
                assert_eq!(
                    actual.stored_set_observation(),
                    expected_set_observation(&expected, &variants)
                );
                assert_eq!(actual.verify_order(), production_order);
                assert_eq!(expected.verify_order(), model_order);
                assert_eq!(
                    expected.entries().values().copied().collect::<Vec<_>>(),
                    variants
                        .iter()
                        .zip(*after)
                        .filter_map(|(variants, state)| model_entry(variants, state))
                        .collect::<Vec<_>>()
                );

                let mut reversed = frontier_from_states(production_order, &variants, *initial);
                let reverse_plan = reversed
                    .plan_batch(variants.iter().enumerate().rev().map(|(index, variants)| {
                        (variants.owner(initial[index]), variants.owner(after[index]))
                    }))
                    .expect("caller order cannot change the scheduler set transition");
                reversed.apply_batch(reverse_plan);
                assert_eq!(actual.snapshot(), reversed.snapshot());
            }
        }
    }
}

#[derive(Clone)]
struct ClassOwners {
    small: OwnedTx,
    large: OwnedTx,
}

impl ClassOwners {
    fn selected(&self, small: bool) -> &OwnedTx {
        if small { &self.small } else { &self.large }
    }
}

#[derive(Clone)]
struct RingOwnerFixtures {
    owner: SchedulerRefinementOwner,
    committed: ClassOwners,
    overlay: ClassOwners,
    seed: ClassOwners,
}

fn queued_verify_owner(
    nonce: u64,
    owner: SchedulerRefinementOwner,
    class: VerifyCycleClass,
) -> OwnedTx {
    let source = match owner {
        SchedulerRefinementOwner::Remote(peer) => SchedulerRefinementSource::Remote(peer),
        SchedulerRefinementOwner::Trusted => SchedulerRefinementSource::Proposal,
    };
    build_variants(nonce, 1, source, owner, class).verify
}

fn ring_bank() -> [RingOwnerFixtures; 3] {
    let mut nonce = 3_000u64;
    let mut next_pair = |owner| {
        nonce += 1;
        let small = queued_verify_owner(nonce, owner, VerifyCycleClass::Small);
        nonce += 1;
        let large = queued_verify_owner(nonce, owner, VerifyCycleClass::Large);
        ClassOwners { small, large }
    };
    [
        SchedulerRefinementOwner::Remote(1),
        SchedulerRefinementOwner::Remote(2),
        SchedulerRefinementOwner::Trusted,
    ]
    .map(|owner| RingOwnerFixtures {
        owner,
        committed: next_pair(owner),
        overlay: next_pair(owner),
        seed: next_pair(owner),
    })
}

#[derive(Clone, Copy, Debug)]
struct Population {
    all: u8,
    small: u8,
}

fn populations() -> Vec<Population> {
    let mut populations = Vec::new();
    for all in 0u8..8 {
        for small in 0u8..8 {
            if small & !all == 0 {
                populations.push(Population { all, small });
            }
        }
    }
    populations
}

fn contains(mask: u8, index: usize) -> bool {
    mask & (1 << index) != 0
}

fn model_population(
    bank: &[RingOwnerFixtures; 3],
    population: Population,
) -> SchedulerOwnerPopulation {
    SchedulerOwnerPopulation::new(
        bank.iter().enumerate().filter_map(|(index, fixture)| {
            contains(population.all, index).then_some(fixture.owner)
        }),
        bank.iter().enumerate().filter_map(|(index, fixture)| {
            contains(population.small, index).then_some(fixture.owner)
        }),
    )
    .expect("the finite small population is a subset of all owners")
}

fn production_owner(owner: SchedulerRefinementOwner) -> WorkOwner {
    match owner {
        SchedulerRefinementOwner::Remote(peer) => {
            WorkOwner::Remote(PeerIndex::from(usize::from(peer)))
        }
        SchedulerRefinementOwner::Trusted => WorkOwner::Trusted,
    }
}

fn population_entries<'bank>(
    bank: &'bank [RingOwnerFixtures; 3],
    population: Population,
    role: impl Fn(&'bank RingOwnerFixtures) -> &'bank ClassOwners,
) -> Vec<&'bank OwnedTx> {
    bank.iter()
        .enumerate()
        .filter(|(index, _)| contains(population.all, *index))
        .map(|(index, fixture)| role(fixture).selected(contains(population.small, index)))
        .collect()
}

fn frontier_with_population(
    bank: &[RingOwnerFixtures; 3],
    population: Population,
    cursor: Option<SchedulerRefinementOwner>,
) -> FairFrontier {
    let committed = population_entries(bank, population, |fixture| &fixture.committed);
    let seed = cursor.map(|cursor| {
        bank.iter()
            .find(|fixture| fixture.owner == cursor)
            .expect("the cursor owner is in the finite universe")
            .seed
            .selected(true)
    });
    let mut frontier = FairFrontier::new(VerifyOrder::Arrival);
    let initial = committed
        .iter()
        .copied()
        .chain(seed)
        .map(|owner| (None, Some(owner)));
    let plan = frontier
        .plan_batch(initial)
        .expect("the committed owner population plans");
    frontier.apply_batch(plan);

    if let Some(seed) = seed {
        let mut wave = frontier
            .checkout_wave(1)
            .expect("one cursor seed reserves one selected version");
        let ticket = frontier
            .ticket_for_foundation(
                &seed.record().identity.raw,
                seed.record().version,
                WorkPermit::VerifyOnly(VerifyCapability::Any),
            )
            .expect("the cursor seed occupies an exact Verify slot");
        wave.select(&ticket)
            .expect("the exact seed advances the cursor once");
        let plan = frontier
            .plan_exchange_batch([(Some(seed), None)], wave)
            .expect("cursor publication and seed removal are one scheduler Apply");
        frontier.apply_batch(plan);
    }
    frontier
}

fn first_unblocked_owner(
    wave: &crate::authority::scheduler::SchedulerExchangeWave<'_>,
    permit: WorkPermit,
    blocked: &BTreeSet<WorkOwner>,
) -> (Option<WorkOwner>, usize) {
    let bound = wave
        .owner_count(permit)
        .expect("the three-owner finite sum cannot overflow");
    let mut cursor = None;
    let mut probes = 0;
    for _ in 0..bound {
        let ticket = match cursor {
            Some(owner) => wave.next_after(permit, owner),
            None => wave.next(permit),
        };
        let Some(ticket) = ticket else {
            return (None, probes);
        };
        probes += 1;
        cursor = Some(ticket.owner());
        if !blocked.contains(&ticket.owner()) {
            return (Some(ticket.owner()), probes);
        }
    }
    (None, probes)
}

#[test]
fn uak_scheduler_owner_ring_refines_every_finite_union_cursor_and_blocked_set() {
    let bank = ring_bank();
    let populations = populations();
    let cursors = [
        None,
        Some(SchedulerRefinementOwner::Remote(1)),
        Some(SchedulerRefinementOwner::Remote(2)),
        Some(SchedulerRefinementOwner::Trusted),
    ];
    for committed in &populations {
        for cursor in cursors {
            let frontier = frontier_with_population(&bank, *committed, cursor);
            for overlay in &populations {
                let overlay_entries =
                    population_entries(&bank, *overlay, |fixture| &fixture.overlay);
                for (small_only, permit) in [
                    (false, WorkPermit::VerifyOnly(VerifyCapability::Any)),
                    (
                        true,
                        WorkPermit::VerifyOnly(VerifyCapability::SmallCycleOnly),
                    ),
                ] {
                    let ring = SchedulerOwnerRing::new(
                        model_population(&bank, *committed),
                        model_population(&bank, *overlay),
                        cursor,
                    );
                    let expected_bound = ring
                        .owner_bound(small_only)
                        .expect("the finite owner sum cannot overflow");
                    for blocked_mask in 0u8..8 {
                        let blocked_model = bank
                            .iter()
                            .enumerate()
                            .filter_map(|(index, fixture)| {
                                contains(blocked_mask, index).then_some(fixture.owner)
                            })
                            .collect::<BTreeSet<_>>();
                        let blocked_production = blocked_model
                            .iter()
                            .copied()
                            .map(production_owner)
                            .collect::<BTreeSet<_>>();
                        let wave = frontier
                            .exchange_wave_after(overlay_entries.iter().copied(), 3)
                            .expect("the overlay population is unique by real owner slot");
                        assert_eq!(
                            wave.owner_count(permit)
                                .expect("the finite production sum cannot overflow"),
                            expected_bound
                        );
                        let (actual, probes) =
                            first_unblocked_owner(&wave, permit, &blocked_production);
                        let expected = ring.first_available(small_only, &blocked_model);
                        assert_eq!(actual.map(refinement_owner), expected);
                        assert!(probes <= expected_bound);
                    }
                }
            }
        }
    }
}
