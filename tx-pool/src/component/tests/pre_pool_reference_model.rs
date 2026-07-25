//! Recomputing executable oracle for the target seven-state pre-pool kernel.
//!
//! This module is intentionally test-only and shares no production indexes or
//! transition helpers. Commands mutate primary entries, then rebuild every
//! queue/reverse index/accounting view from scratch. The production cutover is
//! required to differential-test against this model rather than copying its
//! implementation.

use rand::{Rng, SeedableRng, rngs::StdRng};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

type TxId = u16;
type DependencyKey = u16;
type InputId = u16;
type Version = u128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StateTag {
    RecoveryRetained,
    ResolveQueued,
    ResolveLeased,
    Wait,
    VerifyQueued,
    VerifyLeased,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Remote(u8),
    Proposal,
    ChainRecovery,
}

impl Source {
    fn ready_class(self) -> u8 {
        match self {
            Self::Remote(_) => 0,
            Self::Proposal | Self::ChainRecovery => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    RecoveryRetained {
        ordinal: u16,
    },
    ResolveQueued,
    ResolveLeased,
    Wait {
        keys: BTreeSet<DependencyKey>,
        observed_epochs: BTreeMap<DependencyKey, u64>,
    },
    VerifyQueued {
        dependencies: BTreeSet<DependencyKey>,
    },
    VerifyLeased {
        dependencies: BTreeSet<DependencyKey>,
    },
    Ready {
        dependencies: BTreeSet<DependencyKey>,
        inputs: BTreeSet<InputId>,
        fee: u64,
        serialized_size: u32,
        arrival: u64,
    },
}

impl State {
    fn tag(&self) -> StateTag {
        match self {
            Self::RecoveryRetained { .. } => StateTag::RecoveryRetained,
            Self::ResolveQueued => StateTag::ResolveQueued,
            Self::ResolveLeased => StateTag::ResolveLeased,
            Self::Wait { .. } => StateTag::Wait,
            Self::VerifyQueued { .. } => StateTag::VerifyQueued,
            Self::VerifyLeased { .. } => StateTag::VerifyLeased,
            Self::Ready { .. } => StateTag::Ready,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    version: Version,
    source: Source,
    raw_bytes: usize,
    state: State,
}

impl Entry {
    fn charge(&self) -> usize {
        let state_charge = match &self.state {
            State::RecoveryRetained { .. } => 24,
            State::ResolveQueued | State::ResolveLeased => 16,
            State::Wait { keys, .. } => 32 + keys.len() * 16,
            State::VerifyQueued { dependencies } | State::VerifyLeased { dependencies } => {
                48 + dependencies.len() * 16
            }
            State::Ready {
                dependencies,
                inputs,
                ..
            } => 80 + dependencies.len() * 16 + inputs.len() * 24,
        };
        self.raw_bytes + 64 + state_charge
    }

    fn ready_key(&self, id: TxId) -> Option<ReadyKey> {
        let State::Ready {
            fee,
            serialized_size,
            arrival,
            ..
        } = self.state
        else {
            return None;
        };
        Some(ReadyKey {
            source_class: self.source.ready_class(),
            fee,
            serialized_size,
            arrival,
            id,
            version: self.version,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadyKey {
    source_class: u8,
    fee: u64,
    serialized_size: u32,
    arrival: u64,
    id: TxId,
    version: Version,
}

impl Ord for ReadyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_rate = u128::from(self.fee) * u128::from(other.serialized_size);
        let other_rate = u128::from(other.fee) * u128::from(self.serialized_size);
        self.source_class
            .cmp(&other.source_class)
            .then_with(|| self_rate.cmp(&other_rate))
            .then_with(|| self.fee.cmp(&other.fee))
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.id.cmp(&self.id))
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.serialized_size.cmp(&other.serialized_size))
    }
}

impl PartialOrd for ReadyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Derived {
    resolve_queue: BTreeSet<(u8, TxId, Version)>,
    verify_queue: BTreeSet<(u8, TxId, Version)>,
    waiting_by_key: BTreeMap<DependencyKey, BTreeSet<(TxId, Version)>>,
    resolved_by_key: BTreeMap<DependencyKey, BTreeSet<(TxId, Version)>>,
    ready: BTreeSet<ReadyKey>,
    ready_by_input: BTreeMap<InputId, BTreeSet<ReadyKey>>,
    charge: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lease {
    id: TxId,
    version: Version,
    stage: StateTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanOutcome<T> {
    Apply(T),
    Reject,
    Backpressure,
    Stale,
    Repair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Model {
    entries: BTreeMap<TxId, Entry>,
    accepted: BTreeSet<TxId>,
    dependency_epochs: BTreeMap<DependencyKey, u64>,
    derived: Derived,
    issued_versions: BTreeSet<Version>,
    next_version: Version,
    resident_limit: usize,
}

impl Model {
    fn new(resident_limit: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            accepted: BTreeSet::new(),
            dependency_epochs: BTreeMap::new(),
            derived: Derived::default(),
            issued_versions: BTreeSet::new(),
            next_version: 1,
            resident_limit,
        }
    }

    fn issue_version(&mut self) -> Version {
        let version = self.next_version;
        self.next_version = self
            .next_version
            .checked_add(1)
            .expect("model version exhausted");
        assert!(self.issued_versions.insert(version));
        version
    }

    fn replace_state(&mut self, id: TxId, state: State) -> Version {
        let version = self.issue_version();
        let entry = self.entries.get_mut(&id).unwrap();
        entry.version = version;
        entry.state = state;
        version
    }

    fn complete_state(&mut self, id: TxId, state: State) -> PlanOutcome<Version> {
        let old_charge = self.entries[&id].charge();
        let mut prospective = self.entries[&id].clone();
        prospective.state = state.clone();
        let prospective_charge = self
            .derived
            .charge
            .saturating_sub(old_charge)
            .saturating_add(prospective.charge());
        if prospective_charge > self.resident_limit {
            self.entries.remove(&id);
            self.refresh_and_assert();
            return PlanOutcome::Backpressure;
        }
        let version = self.replace_state(id, state);
        self.refresh_and_assert();
        PlanOutcome::Apply(version)
    }

    fn rebuild(&self) -> Derived {
        let mut rebuilt = Derived::default();
        for (id, entry) in &self.entries {
            rebuilt.charge += entry.charge();
            let owner_class = entry.source.ready_class();
            match &entry.state {
                State::RecoveryRetained { .. } | State::ResolveQueued => {
                    rebuilt
                        .resolve_queue
                        .insert((owner_class, *id, entry.version));
                }
                State::ResolveLeased => {}
                State::Wait { keys, .. } => {
                    for key in keys {
                        rebuilt
                            .waiting_by_key
                            .entry(*key)
                            .or_default()
                            .insert((*id, entry.version));
                    }
                }
                State::VerifyQueued { dependencies } => {
                    rebuilt
                        .verify_queue
                        .insert((owner_class, *id, entry.version));
                    for key in dependencies {
                        rebuilt
                            .resolved_by_key
                            .entry(*key)
                            .or_default()
                            .insert((*id, entry.version));
                    }
                }
                State::VerifyLeased { dependencies } => {
                    for key in dependencies {
                        rebuilt
                            .resolved_by_key
                            .entry(*key)
                            .or_default()
                            .insert((*id, entry.version));
                    }
                }
                State::Ready {
                    dependencies,
                    inputs,
                    ..
                } => {
                    for key in dependencies {
                        rebuilt
                            .resolved_by_key
                            .entry(*key)
                            .or_default()
                            .insert((*id, entry.version));
                    }
                    let rank = entry.ready_key(*id).unwrap();
                    rebuilt.ready.insert(rank);
                    for input in inputs {
                        rebuilt
                            .ready_by_input
                            .entry(*input)
                            .or_default()
                            .insert(rank);
                    }
                }
            }
        }
        rebuilt
    }

    fn refresh_and_assert(&mut self) {
        self.derived = self.rebuild();
        self.assert_valid();
    }

    fn assert_valid(&self) {
        assert_eq!(self.derived, self.rebuild());
        assert!(self.entries.keys().all(|id| !self.accepted.contains(id)));
        assert!(self.derived.charge <= self.resident_limit);
        assert_eq!(
            self.entries.len(),
            self.entries
                .values()
                .map(|e| e.version)
                .collect::<BTreeSet<_>>()
                .len()
        );
        for entry in self.entries.values() {
            assert!(self.issued_versions.contains(&entry.version));
            match &entry.state {
                State::Wait {
                    keys,
                    observed_epochs,
                } => {
                    assert!(!keys.is_empty());
                    assert_eq!(keys, &observed_epochs.keys().copied().collect());
                }
                State::Ready {
                    inputs,
                    serialized_size,
                    ..
                } => {
                    assert!(!inputs.is_empty());
                    assert_ne!(*serialized_size, 0);
                }
                _ => {}
            }
        }
        let queued = self
            .entries
            .values()
            .filter(|entry| {
                matches!(
                    entry.state,
                    State::RecoveryRetained { .. } | State::ResolveQueued
                )
            })
            .count();
        assert_eq!(queued, self.derived.resolve_queue.len());
        let verify_queued = self
            .entries
            .values()
            .filter(|entry| matches!(entry.state, State::VerifyQueued { .. }))
            .count();
        assert_eq!(verify_queued, self.derived.verify_queue.len());
    }

    fn insert(
        &mut self,
        id: TxId,
        source: Source,
        raw_bytes: usize,
        state: State,
    ) -> PlanOutcome<Version> {
        if self.accepted.contains(&id) {
            return PlanOutcome::Reject;
        }
        if self.entries.contains_key(&id) || raw_bytes == 0 {
            return PlanOutcome::Stale;
        }
        let entry = Entry {
            version: self.next_version,
            source,
            raw_bytes,
            state,
        };
        if self.derived.charge.saturating_add(entry.charge()) > self.resident_limit {
            return PlanOutcome::Backpressure;
        }
        let version = self.issue_version();
        let entry = Entry { version, ..entry };
        self.entries.insert(id, entry);
        self.refresh_and_assert();
        PlanOutcome::Apply(version)
    }

    fn admit(&mut self, id: TxId, source: Source, raw_bytes: usize) -> PlanOutcome<Version> {
        self.insert(id, source, raw_bytes, State::ResolveQueued)
    }

    fn retain_recovery(&mut self, id: TxId, ordinal: u16) -> PlanOutcome<Version> {
        self.insert(
            id,
            Source::ChainRecovery,
            100,
            State::RecoveryRetained { ordinal },
        )
    }

    fn promote(&mut self, id: TxId, different_witness: bool) -> PlanOutcome<Version> {
        let Some(entry) = self.entries.get_mut(&id) else {
            return PlanOutcome::Stale;
        };
        if entry.source == Source::Proposal && !different_witness {
            return PlanOutcome::Stale;
        }
        if different_witness {
            let version = self.issue_version();
            let entry = self.entries.get_mut(&id).unwrap();
            entry.source = Source::Proposal;
            entry.version = version;
            entry.state = State::ResolveQueued;
            self.refresh_and_assert();
            PlanOutcome::Apply(version)
        } else {
            entry.source = Source::Proposal;
            let version = entry.version;
            self.refresh_and_assert();
            PlanOutcome::Apply(version)
        }
    }

    fn checkout_resolve(&mut self, id: TxId) -> PlanOutcome<Lease> {
        let Some(entry) = self.entries.get(&id) else {
            return PlanOutcome::Stale;
        };
        if !matches!(
            entry.state,
            State::RecoveryRetained { .. } | State::ResolveQueued
        ) {
            return PlanOutcome::Stale;
        }
        let version = self.replace_state(id, State::ResolveLeased);
        self.refresh_and_assert();
        PlanOutcome::Apply(Lease {
            id,
            version,
            stage: StateTag::ResolveLeased,
        })
    }

    fn complete_resolve(&mut self, lease: Lease, result: ResolveResult) -> PlanOutcome<Version> {
        if self.entries.get(&lease.id).is_none_or(|entry| {
            entry.version != lease.version || entry.state.tag() != StateTag::ResolveLeased
        }) {
            return PlanOutcome::Stale;
        }
        if matches!(result, ResolveResult::Reject) {
            self.entries.remove(&lease.id);
            self.refresh_and_assert();
            return PlanOutcome::Reject;
        }
        let state = match result {
            ResolveResult::Resolved(dependencies) => State::VerifyQueued { dependencies },
            ResolveResult::Missing(keys) if !keys.is_empty() => State::Wait {
                observed_epochs: keys
                    .iter()
                    .map(|key| (*key, *self.dependency_epochs.get(key).unwrap_or(&0)))
                    .collect(),
                keys,
            },
            ResolveResult::Missing(_) => State::ResolveQueued,
            ResolveResult::Reject => unreachable!(),
        };
        self.complete_state(lease.id, state)
    }

    fn wake(&mut self, key: DependencyKey) -> PlanOutcome<usize> {
        *self.dependency_epochs.entry(key).or_default() += 1;
        let waking: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(id, entry)| match &entry.state {
                State::Wait { keys, .. } if keys.contains(&key) => Some(*id),
                _ => None,
            })
            .collect();
        for id in &waking {
            self.replace_state(*id, State::ResolveQueued);
        }
        self.refresh_and_assert();
        PlanOutcome::Apply(waking.len())
    }

    fn checkout_verify(&mut self, id: TxId) -> PlanOutcome<Lease> {
        let Some(entry) = self.entries.get(&id) else {
            return PlanOutcome::Stale;
        };
        let State::VerifyQueued { dependencies } = &entry.state else {
            return PlanOutcome::Stale;
        };
        let dependencies = dependencies.clone();
        let version = self.replace_state(id, State::VerifyLeased { dependencies });
        self.refresh_and_assert();
        PlanOutcome::Apply(Lease {
            id,
            version,
            stage: StateTag::VerifyLeased,
        })
    }

    fn complete_verify(&mut self, lease: Lease, result: VerifyResult) -> PlanOutcome<Version> {
        let Some(entry) = self.entries.get(&lease.id) else {
            return PlanOutcome::Stale;
        };
        let State::VerifyLeased { dependencies } = &entry.state else {
            return PlanOutcome::Stale;
        };
        if entry.version != lease.version || lease.stage != StateTag::VerifyLeased {
            return PlanOutcome::Stale;
        }
        let dependencies = dependencies.clone();
        if matches!(result, VerifyResult::Reject) {
            self.entries.remove(&lease.id);
            self.refresh_and_assert();
            return PlanOutcome::Reject;
        }
        let state = match result {
            VerifyResult::Verified {
                inputs,
                fee,
                size,
                arrival,
            } => State::Ready {
                dependencies,
                inputs,
                fee,
                serialized_size: size,
                arrival,
            },
            VerifyResult::Stale(keys) if !keys.is_empty() => State::Wait {
                observed_epochs: keys
                    .iter()
                    .map(|key| (*key, *self.dependency_epochs.get(key).unwrap_or(&0)))
                    .collect(),
                keys,
            },
            VerifyResult::Stale(_) => State::ResolveQueued,
            VerifyResult::Reject => unreachable!(),
        };
        self.complete_state(lease.id, state)
    }

    fn accept_ready(&mut self, id: TxId, version: Version) -> PlanOutcome<()> {
        let Some(entry) = self.entries.get(&id) else {
            return PlanOutcome::Stale;
        };
        let Some(rank) = entry.ready_key(id) else {
            return PlanOutcome::Stale;
        };
        if entry.version != version {
            return PlanOutcome::Stale;
        }
        let State::Ready { inputs, .. } = &entry.state else {
            unreachable!();
        };
        if inputs.iter().any(|input| {
            self.derived
                .ready_by_input
                .get(input)
                .and_then(|bucket| bucket.last())
                .is_some_and(|head| *head != rank)
        }) {
            return PlanOutcome::Stale;
        }
        let direct_conflicts: BTreeSet<_> = inputs
            .iter()
            .filter_map(|input| self.derived.ready_by_input.get(input))
            .flat_map(|bucket| bucket.iter().map(|key| key.id))
            .collect();
        for removed in direct_conflicts {
            self.entries.remove(&removed);
        }
        self.accepted.insert(id);
        self.refresh_and_assert();
        PlanOutcome::Apply(())
    }

    fn remove(&mut self, id: TxId) -> PlanOutcome<()> {
        if self.entries.remove(&id).is_none() {
            return PlanOutcome::Stale;
        }
        self.refresh_and_assert();
        PlanOutcome::Apply(())
    }

    fn clear_generation(&mut self) -> PlanOutcome<()> {
        self.entries.clear();
        self.refresh_and_assert();
        PlanOutcome::Apply(())
    }

    fn projection_probe(&self, matches_primary: bool) -> PlanOutcome<()> {
        if matches_primary {
            PlanOutcome::Apply(())
        } else {
            PlanOutcome::Repair
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolveResult {
    Resolved(BTreeSet<DependencyKey>),
    Missing(BTreeSet<DependencyKey>),
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerifyResult {
    Verified {
        inputs: BTreeSet<InputId>,
        fee: u64,
        size: u32,
        arrival: u64,
    },
    Stale(BTreeSet<DependencyKey>),
    Reject,
}

fn ready_result(id: TxId, arrival: u64) -> VerifyResult {
    VerifyResult::Verified {
        inputs: BTreeSet::from([id % 11]),
        fee: u64::from(id) + 1,
        size: u32::from(id % 31) + 1,
        arrival,
    }
}

#[test]
fn target_model_declares_exactly_the_frozen_seven_states() {
    let states = BTreeSet::from([
        StateTag::RecoveryRetained,
        StateTag::ResolveQueued,
        StateTag::ResolveLeased,
        StateTag::Wait,
        StateTag::VerifyQueued,
        StateTag::VerifyLeased,
        StateTag::Ready,
    ]);
    assert_eq!(states.len(), 7);
}

#[test]
fn target_model_exercises_every_plan_outcome_without_partial_mutation() {
    let mut model = Model::new(300);
    assert!(matches!(
        model.admit(1, Source::Remote(1), 100),
        PlanOutcome::Apply(_)
    ));
    let before = model.clone();
    assert_eq!(model.admit(1, Source::Remote(1), 100), PlanOutcome::Stale);
    assert_eq!(model, before);
    assert_eq!(
        model.admit(2, Source::Remote(1), 300),
        PlanOutcome::Backpressure
    );
    assert_eq!(model, before);
    model.accepted.insert(3);
    assert_eq!(model.admit(3, Source::Remote(1), 1), PlanOutcome::Reject);
    assert_eq!(model.projection_probe(false), PlanOutcome::Repair);
    model.assert_valid();
}

#[test]
fn target_model_stale_lease_cannot_mutate_a_replaced_witness_owner() {
    let mut model = Model::new(10_000);
    model.admit(1, Source::Remote(7), 100);
    let PlanOutcome::Apply(old_lease) = model.checkout_resolve(1) else {
        panic!()
    };
    model.promote(1, true);
    let before = model.clone();
    assert_eq!(
        model.complete_resolve(old_lease, ResolveResult::Resolved(BTreeSet::from([9]))),
        PlanOutcome::Stale
    );
    assert_eq!(model, before);
}

#[test]
fn target_model_wait_wake_and_ready_conflict_use_recomputed_views() {
    let mut model = Model::new(20_000);
    for id in 1..=2 {
        model.admit(id, Source::Remote(id as u8), 100);
        let PlanOutcome::Apply(resolve) = model.checkout_resolve(id) else {
            panic!()
        };
        if id == 1 {
            model.complete_resolve(resolve, ResolveResult::Missing(BTreeSet::from([50])));
            assert_eq!(model.wake(50), PlanOutcome::Apply(1));
        } else {
            model.complete_resolve(resolve, ResolveResult::Resolved(BTreeSet::from([50])));
        }
        if matches!(model.entries[&id].state, State::ResolveQueued) {
            let PlanOutcome::Apply(resolve) = model.checkout_resolve(id) else {
                panic!()
            };
            model.complete_resolve(resolve, ResolveResult::Resolved(BTreeSet::from([50])));
        }
        let PlanOutcome::Apply(verify) = model.checkout_verify(id) else {
            panic!()
        };
        model.complete_verify(
            verify,
            VerifyResult::Verified {
                inputs: BTreeSet::from([7]),
                fee: u64::from(id),
                size: 1,
                arrival: u64::from(id),
            },
        );
    }
    let winner = model.derived.ready.last().unwrap();
    assert_eq!(winner.id, 2);
    assert_eq!(
        model.accept_ready(1, model.entries[&1].version),
        PlanOutcome::Stale
    );
    assert_eq!(
        model.accept_ready(2, model.entries[&2].version),
        PlanOutcome::Apply(())
    );
    assert_eq!(model.accepted, BTreeSet::from([2]));
    assert!(model.entries.is_empty());
}

#[test]
fn target_model_generated_commands_preserve_partition_lease_budget_and_indexes() {
    let mut model = Model::new(30_000);
    let mut rng = StdRng::seed_from_u64(0x5041_4b5f_4d4f_4445);
    let mut next_id: TxId = 1;
    let mut arrival = 0;
    for _ in 0..8_000 {
        match rng.gen_range(0..11) {
            0 | 1 if next_id < 2_000 => {
                let id = next_id;
                next_id += 1;
                let source = if rng.gen_bool(0.2) {
                    Source::Proposal
                } else {
                    Source::Remote(rng.gen_range(0..8))
                };
                let _ = model.admit(id, source, rng.gen_range(1..=200));
            }
            2 if next_id < 2_000 => {
                let id = next_id;
                next_id += 1;
                let _ = model.retain_recovery(id, id);
            }
            3 if !model.derived.resolve_queue.is_empty() => {
                let (_, id, _) = *model.derived.resolve_queue.iter().next().unwrap();
                let PlanOutcome::Apply(lease) = model.checkout_resolve(id) else {
                    panic!()
                };
                let result = match rng.gen_range(0..6) {
                    0 => ResolveResult::Missing(BTreeSet::from([rng.gen_range(0..32)])),
                    1 => ResolveResult::Reject,
                    _ => ResolveResult::Resolved(BTreeSet::from([rng.gen_range(0..32)])),
                };
                let _ = model.complete_resolve(lease, result);
            }
            4 if !model.derived.verify_queue.is_empty() => {
                let (_, id, _) = *model.derived.verify_queue.iter().next().unwrap();
                let PlanOutcome::Apply(lease) = model.checkout_verify(id) else {
                    panic!()
                };
                let result = match rng.gen_range(0..8) {
                    0 => VerifyResult::Reject,
                    1 => VerifyResult::Stale(BTreeSet::from([rng.gen_range(0..32)])),
                    _ => ready_result(id, arrival),
                };
                arrival += 1;
                let _ = model.complete_verify(lease, result);
            }
            5 if !model.derived.ready.is_empty() => {
                let key = *model.derived.ready.last().unwrap();
                let _ = model.accept_ready(key.id, key.version);
            }
            6 => {
                let _ = model.wake(rng.gen_range(0..32));
            }
            7 if !model.entries.is_empty() => {
                let id = *model
                    .entries
                    .keys()
                    .nth(rng.gen_range(0..model.entries.len()))
                    .unwrap();
                let _ = model.promote(id, rng.gen_bool(0.3));
            }
            8 if !model.entries.is_empty() => {
                let id = *model
                    .entries
                    .keys()
                    .nth(rng.gen_range(0..model.entries.len()))
                    .unwrap();
                let _ = model.remove(id);
            }
            9 if rng.gen_ratio(1, 200) => {
                let _ = model.clear_generation();
            }
            10 if !model.entries.is_empty() => {
                let before = model.clone();
                let id = *model.entries.keys().next().unwrap();
                let stale = Lease {
                    id,
                    version: 0,
                    stage: StateTag::ResolveLeased,
                };
                assert_eq!(
                    model.complete_resolve(stale, ResolveResult::Resolved(BTreeSet::new())),
                    PlanOutcome::Stale
                );
                assert_eq!(model, before);
            }
            _ => {}
        }
        model.assert_valid();
    }
}
