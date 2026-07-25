//! Executable reference model for the thin pre-pool conflict contract.
//!
//! This intentionally does not use the production coordinator indexes. It
//! gives the Phase-2 cutover a small, deterministic oracle for graph, rank,
//! ticket and direct-handoff semantics.

use rand::{Rng, SeedableRng, rngs::StdRng};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

type CandidateId = u16;
type InputId = u16;
type RebuiltConflictState = (
    BTreeMap<InputId, BTreeSet<CandidateId>>,
    BTreeMap<CandidateId, ModelRelation>,
    BTreeSet<CandidateId>,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ModelSource {
    Remote,
    Local,
    Proposal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelState {
    Verified,
    Committing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelCandidate {
    source: ModelSource,
    fee: u64,
    tx_size: usize,
    arrival: u64,
    inputs: BTreeSet<InputId>,
    state: ModelState,
}

impl ModelCandidate {
    fn rank(&self, id: CandidateId) -> ModelRank {
        ModelRank {
            committing: self.state == ModelState::Committing,
            source: self.source,
            fee: self.fee,
            tx_size: self.tx_size,
            arrival: self.arrival,
            id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModelRank {
    committing: bool,
    source: ModelSource,
    fee: u64,
    tx_size: usize,
    arrival: u64,
    id: CandidateId,
}

impl Ord for ModelRank {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_rate = u128::from(self.fee) * other.tx_size as u128;
        let other_rate = u128::from(other.fee) * self.tx_size as u128;
        self.committing
            .cmp(&other.committing)
            .then_with(|| self.source.cmp(&other.source))
            .then_with(|| self_rate.cmp(&other_rate))
            .then_with(|| self.fee.cmp(&other.fee))
            // Earlier arrival and then the smaller stable identity win.
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.id.cmp(&self.id))
            // Preserve the Ord/Eq contract for zero-fee synthetic ranks with
            // otherwise identical fields. Production identity is unique.
            .then_with(|| self.tx_size.cmp(&other.tx_size))
    }
}

impl PartialOrd for ModelRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModelRelation {
    degree: usize,
    stronger_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConflictModel {
    entries: BTreeMap<CandidateId, ModelCandidate>,
    by_input: BTreeMap<InputId, BTreeSet<CandidateId>>,
    relations: BTreeMap<CandidateId, ModelRelation>,
    live_tickets: BTreeSet<CandidateId>,
    max_direct_conflicts: usize,
    max_candidates_per_input: usize,
    input_bucket_probes: usize,
}

impl ConflictModel {
    fn new(max_direct_conflicts: usize, max_candidates_per_input: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            by_input: BTreeMap::new(),
            relations: BTreeMap::new(),
            live_tickets: BTreeSet::new(),
            max_direct_conflicts,
            max_candidates_per_input,
            input_bucket_probes: 0,
        }
    }

    fn direct_neighbours(
        &mut self,
        id: CandidateId,
        inputs: &BTreeSet<InputId>,
    ) -> BTreeSet<CandidateId> {
        let mut neighbours = BTreeSet::new();
        for input in inputs {
            if let Some(bucket) = self.by_input.get(input) {
                self.input_bucket_probes += bucket.len();
                neighbours.extend(bucket.iter().copied().filter(|other| *other != id));
            }
        }
        neighbours
    }

    fn neighbours_of(&mut self, id: CandidateId) -> BTreeSet<CandidateId> {
        let inputs = self.entries[&id].inputs.clone();
        self.direct_neighbours(id, &inputs)
    }

    fn refresh_ticket(&mut self, id: CandidateId) {
        let eligible = self.entries.get(&id).is_some_and(|entry| {
            entry.state == ModelState::Verified && self.relations[&id].stronger_count == 0
        });
        if eligible {
            self.live_tickets.insert(id);
        } else {
            self.live_tickets.remove(&id);
        }
    }

    fn insert(&mut self, id: CandidateId, candidate: ModelCandidate) -> Result<(), ()> {
        if self.entries.contains_key(&id)
            || candidate.inputs.is_empty()
            || candidate.tx_size == 0
            || candidate.state != ModelState::Verified
            || candidate.inputs.iter().any(|input| {
                self.by_input.get(input).map_or(0, BTreeSet::len) >= self.max_candidates_per_input
            })
        {
            return Err(());
        }
        let neighbours = self.direct_neighbours(id, &candidate.inputs);
        if neighbours.len() > self.max_direct_conflicts
            || neighbours
                .iter()
                .any(|neighbour| self.relations[neighbour].degree >= self.max_direct_conflicts)
        {
            return Err(());
        }
        let incoming_rank = candidate.rank(id);
        self.entries.insert(id, candidate);
        self.relations.insert(id, ModelRelation::default());
        for neighbour in &neighbours {
            self.relations.get_mut(&id).unwrap().degree += 1;
            self.relations.get_mut(neighbour).unwrap().degree += 1;
            if incoming_rank > self.entries[neighbour].rank(*neighbour) {
                self.relations.get_mut(neighbour).unwrap().stronger_count += 1;
            } else {
                self.relations.get_mut(&id).unwrap().stronger_count += 1;
            }
        }
        for input in &self.entries[&id].inputs {
            self.by_input.entry(*input).or_default().insert(id);
        }
        self.refresh_ticket(id);
        for neighbour in neighbours {
            self.refresh_ticket(neighbour);
        }
        Ok(())
    }

    fn rerank(
        &mut self,
        id: CandidateId,
        mutate: impl FnOnce(&mut ModelCandidate),
    ) -> Result<(), ()> {
        let neighbours = self.neighbours_of(id);
        let old_rank = self.entries.get(&id).ok_or(())?.rank(id);
        mutate(self.entries.get_mut(&id).ok_or(())?);
        let new_rank = self.entries[&id].rank(id);
        for neighbour in &neighbours {
            let neighbour_rank = self.entries[neighbour].rank(*neighbour);
            match (old_rank.cmp(&neighbour_rank), new_rank.cmp(&neighbour_rank)) {
                (Ordering::Greater, Ordering::Less) => {
                    self.relations.get_mut(neighbour).unwrap().stronger_count -= 1;
                    self.relations.get_mut(&id).unwrap().stronger_count += 1;
                }
                (Ordering::Less, Ordering::Greater) => {
                    self.relations.get_mut(&id).unwrap().stronger_count -= 1;
                    self.relations.get_mut(neighbour).unwrap().stronger_count += 1;
                }
                _ => {}
            }
        }
        self.refresh_ticket(id);
        for neighbour in neighbours {
            self.refresh_ticket(neighbour);
        }
        Ok(())
    }

    fn begin_commit(&mut self, id: CandidateId) -> Result<(), ()> {
        if self
            .entries
            .values()
            .any(|entry| entry.state == ModelState::Committing)
            || !self.live_tickets.contains(&id)
        {
            return Err(());
        }
        self.rerank(id, |entry| entry.state = ModelState::Committing)
    }

    fn abort_commit(&mut self, id: CandidateId) -> Result<(), ()> {
        if self.entries.get(&id).map(|entry| entry.state) != Some(ModelState::Committing) {
            return Err(());
        }
        self.rerank(id, |entry| entry.state = ModelState::Verified)
    }

    fn remove_many(&mut self, removed: &BTreeSet<CandidateId>) -> Result<(), ()> {
        if removed.iter().any(|id| !self.entries.contains_key(id)) {
            return Err(());
        }
        let mut surviving_neighbours = BTreeSet::new();
        let mut edges = Vec::new();
        for id in removed {
            for neighbour in self.neighbours_of(*id) {
                if !removed.contains(&neighbour) {
                    surviving_neighbours.insert(neighbour);
                    edges.push((*id, neighbour));
                }
            }
        }
        for (id, neighbour) in edges {
            let removed_rank = self.entries[&id].rank(id);
            let neighbour_rank = self.entries[&neighbour].rank(neighbour);
            let relation = self.relations.get_mut(&neighbour).unwrap();
            relation.degree -= 1;
            if removed_rank > neighbour_rank {
                relation.stronger_count -= 1;
            }
        }
        for id in removed {
            let entry = self.entries.remove(id).unwrap();
            for input in entry.inputs {
                let bucket = self.by_input.get_mut(&input).unwrap();
                bucket.remove(id);
                if bucket.is_empty() {
                    self.by_input.remove(&input);
                }
            }
            self.relations.remove(id);
            self.live_tickets.remove(id);
        }
        for neighbour in surviving_neighbours {
            self.refresh_ticket(neighbour);
        }
        Ok(())
    }

    fn commit_success(&mut self, id: CandidateId) -> Result<BTreeSet<CandidateId>, ()> {
        if self.entries.get(&id).map(|entry| entry.state) != Some(ModelState::Committing) {
            return Err(());
        }
        let mut removed = self.neighbours_of(id);
        removed.insert(id);
        self.remove_many(&removed)?;
        Ok(removed)
    }

    fn remove_one(&mut self, id: CandidateId) -> Result<(), ()> {
        self.remove_many(&BTreeSet::from([id]))
    }

    fn rebuild(&self) -> RebuiltConflictState {
        let mut by_input: BTreeMap<InputId, BTreeSet<CandidateId>> = BTreeMap::new();
        let mut relations: BTreeMap<CandidateId, ModelRelation> = self
            .entries
            .keys()
            .map(|id| (*id, ModelRelation::default()))
            .collect();
        for (id, entry) in &self.entries {
            for input in &entry.inputs {
                by_input.entry(*input).or_default().insert(*id);
            }
        }
        let ids: Vec<_> = self.entries.keys().copied().collect();
        for (offset, left) in ids.iter().enumerate() {
            for right in ids.iter().skip(offset + 1) {
                if self.entries[left]
                    .inputs
                    .is_disjoint(&self.entries[right].inputs)
                {
                    continue;
                }
                relations.get_mut(left).unwrap().degree += 1;
                relations.get_mut(right).unwrap().degree += 1;
                if self.entries[left].rank(*left) > self.entries[right].rank(*right) {
                    relations.get_mut(right).unwrap().stronger_count += 1;
                } else {
                    relations.get_mut(left).unwrap().stronger_count += 1;
                }
            }
        }
        let tickets = self
            .entries
            .iter()
            .filter_map(|(id, entry)| {
                (entry.state == ModelState::Verified && relations[id].stronger_count == 0)
                    .then_some(*id)
            })
            .collect();
        (by_input, relations, tickets)
    }

    fn assert_valid(&self) {
        let (by_input, relations, tickets) = self.rebuild();
        assert_eq!(self.by_input, by_input);
        assert_eq!(self.relations, relations);
        assert_eq!(self.live_tickets, tickets);
        for id in &tickets {
            for other in &tickets {
                if id < other {
                    assert!(
                        self.entries[id]
                            .inputs
                            .is_disjoint(&self.entries[other].inputs)
                    );
                }
            }
        }
        let committing = self
            .entries
            .values()
            .filter(|entry| entry.state == ModelState::Committing)
            .count();
        assert!(committing <= 1);
        if committing == 0
            && self
                .entries
                .values()
                .any(|entry| entry.state == ModelState::Verified)
        {
            assert!(!tickets.is_empty());
        }
    }
}

fn candidate(
    source: ModelSource,
    fee: u64,
    tx_size: usize,
    arrival: u64,
    inputs: impl IntoIterator<Item = InputId>,
) -> ModelCandidate {
    ModelCandidate {
        source,
        fee,
        tx_size,
        arrival,
        inputs: inputs.into_iter().collect(),
        state: ModelState::Verified,
    }
}

#[test]
fn model_rank_is_total_and_compares_fee_rate_without_division() {
    let higher_rate = candidate(ModelSource::Remote, 2, 3, 0, [1]).rank(1);
    let lower_rate = candidate(ModelSource::Remote, 3, 5, 0, [1]).rank(2);
    assert!(higher_rate > lower_rate);
    assert!(candidate(ModelSource::Proposal, 0, 100, 9, [1]).rank(3) > higher_rate);
    assert!(candidate(ModelSource::Local, 2, 3, 0, [1]).rank(4) > higher_rate);
    assert!(
        candidate(ModelSource::Remote, 2, 3, 0, [1]).rank(1)
            > candidate(ModelSource::Remote, 2, 3, 1, [1]).rank(2)
    );
}

#[test]
fn model_allows_multiple_nonadjacent_local_maxima() {
    let mut model = ConflictModel::new(100, 100);
    model
        .insert(1, candidate(ModelSource::Proposal, 1, 1, 0, [10]))
        .unwrap();
    model
        .insert(2, candidate(ModelSource::Remote, 1, 1, 1, [10, 20]))
        .unwrap();
    model
        .insert(3, candidate(ModelSource::Local, 1, 1, 2, [20]))
        .unwrap();
    model.assert_valid();
    assert_eq!(model.live_tickets, BTreeSet::from([1, 3]));
}

#[test]
fn model_committing_freezes_direct_conflicts_until_abort() {
    let mut model = ConflictModel::new(100, 100);
    model
        .insert(1, candidate(ModelSource::Local, 10, 10, 0, [10]))
        .unwrap();
    model.begin_commit(1).unwrap();
    model
        .insert(2, candidate(ModelSource::Proposal, 100, 10, 1, [10]))
        .unwrap();
    model.assert_valid();
    assert!(model.live_tickets.is_empty());
    model.abort_commit(1).unwrap();
    model.assert_valid();
    assert_eq!(model.live_tickets, BTreeSet::from([2]));
}

#[test]
fn model_success_removes_only_winner_and_direct_conflicts() {
    let mut model = ConflictModel::new(100, 100);
    model
        .insert(1, candidate(ModelSource::Proposal, 3, 1, 0, [10]))
        .unwrap();
    model
        .insert(2, candidate(ModelSource::Local, 2, 1, 1, [10, 20]))
        .unwrap();
    model
        .insert(3, candidate(ModelSource::Remote, 1, 1, 2, [20]))
        .unwrap();
    model.begin_commit(1).unwrap();
    let removed = model.commit_success(1).unwrap();
    model.assert_valid();
    assert_eq!(removed, BTreeSet::from([1, 2]));
    assert_eq!(model.live_tickets, BTreeSet::from([3]));
}

#[test]
fn model_rejects_oversized_direct_cohort_without_partial_mutation() {
    let mut model = ConflictModel::new(2, 10);
    for id in 1..=3 {
        model
            .insert(
                id,
                candidate(ModelSource::Remote, u64::from(id), 1, u64::from(id), [10]),
            )
            .unwrap();
    }
    let before = model.clone();
    assert_eq!(
        model.insert(4, candidate(ModelSource::Proposal, 100, 1, 4, [10])),
        Err(())
    );
    let probes = model.input_bucket_probes;
    model.input_bucket_probes = before.input_bucket_probes;
    assert_eq!(model, before);
    assert_eq!(probes - before.input_bucket_probes, 3);
}

#[test]
fn model_conflict_probe_cost_ignores_independent_population() {
    let mut model = ConflictModel::new(100, 100);
    for id in 1..=1_000 {
        model
            .insert(
                id,
                candidate(ModelSource::Remote, 1, 1, u64::from(id), [id + 2_000]),
            )
            .unwrap();
    }
    let before = model.input_bucket_probes;
    model
        .insert(1_500, candidate(ModelSource::Local, 2, 1, 1_500, [2_001]))
        .unwrap();
    assert_eq!(model.input_bucket_probes - before, 1);
    model.assert_valid();
}

#[test]
fn model_random_transitions_always_match_full_rebuild() {
    let mut model = ConflictModel::new(12, 12);
    let mut rng = StdRng::seed_from_u64(0x0043_4b42_504f_4f4c);
    let mut next_id: CandidateId = 1;
    let mut next_arrival = 0u64;
    for _ in 0..4_000 {
        match rng.gen_range(0..6) {
            0 | 1 if next_id < 2_000 => {
                let id = next_id;
                next_id += 1;
                let input_count = rng.gen_range(1..=3);
                let inputs = (0..input_count)
                    .map(|_| rng.gen_range(0..32))
                    .collect::<BTreeSet<_>>();
                let source = match rng.gen_range(0..3) {
                    0 => ModelSource::Remote,
                    1 => ModelSource::Local,
                    _ => ModelSource::Proposal,
                };
                let _ = model.insert(
                    id,
                    candidate(
                        source,
                        rng.gen_range(0..10_000),
                        rng.gen_range(1..=2_000),
                        next_arrival,
                        inputs,
                    ),
                );
                next_arrival += 1;
            }
            2 if !model.entries.is_empty() => {
                let index = rng.gen_range(0..model.entries.len());
                let id = *model.entries.keys().nth(index).unwrap();
                if model.entries[&id].state == ModelState::Verified {
                    let target = if rng.gen_bool(0.5) {
                        ModelSource::Local
                    } else {
                        ModelSource::Proposal
                    };
                    let _ = model.rerank(id, |entry| {
                        if target > entry.source {
                            entry.source = target;
                        }
                    });
                }
            }
            3 if !model.live_tickets.is_empty()
                && !model
                    .entries
                    .values()
                    .any(|entry| entry.state == ModelState::Committing) =>
            {
                let index = rng.gen_range(0..model.live_tickets.len());
                let id = *model.live_tickets.iter().nth(index).unwrap();
                model.begin_commit(id).unwrap();
            }
            4 => {
                if let Some(id) = model
                    .entries
                    .iter()
                    .find_map(|(id, entry)| (entry.state == ModelState::Committing).then_some(*id))
                {
                    if rng.gen_bool(0.5) {
                        model.abort_commit(id).unwrap();
                    } else {
                        model.commit_success(id).unwrap();
                    }
                }
            }
            5 if !model.entries.is_empty() => {
                let removable: Vec<_> = model
                    .entries
                    .iter()
                    .filter_map(|(id, entry)| (entry.state == ModelState::Verified).then_some(*id))
                    .collect();
                if !removable.is_empty() {
                    model
                        .remove_one(removable[rng.gen_range(0..removable.len())])
                        .unwrap();
                }
            }
            _ => {}
        }
        model.assert_valid();
    }
}
