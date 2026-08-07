//! Executable negative witnesses for the immutable `develop` comparison cut.
//!
//! This is not a second normative tx-pool model. Each transition below is a
//! deliberately small projection of an ordering that the source gate derives
//! from `develop@91b97ab5f`. The positive authority model remains the semantic
//! specification; these witnesses establish only why the corresponding
//! legacy ordering cannot prove the same observation.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Tx(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Peer(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Location {
    VerifyQueue,
    ActiveVerifier,
    Orphan,
    Accepted,
    ConflictCache,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PoolStatus {
    Pending,
    Gap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Publication {
    MembershipCommitted,
    RequiredEffectCommitted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CacheIdentity {
    Raw([u8; 4]),
    Witness([u8; 4]),
}

#[derive(Default)]
struct DevelopWitness {
    locations: BTreeMap<Tx, BTreeSet<Location>>,
    peers: BTreeMap<Tx, Peer>,
    bytes: BTreeMap<Tx, u64>,
    publications: BTreeSet<(Tx, Publication)>,
    missing_parent: BTreeMap<Tx, Tx>,
    pending_orphan_wakes: BTreeSet<Tx>,
    statuses: BTreeMap<Tx, PoolStatus>,
    selected_uncle_proposals: BTreeSet<Tx>,
    detached_recovery: BTreeSet<Tx>,
    cache: BTreeSet<CacheIdentity>,
}

impl DevelopWitness {
    fn add_location(&mut self, transaction: Tx, location: Location) {
        self.locations
            .entry(transaction)
            .or_default()
            .insert(location);
    }

    fn remove_location(&mut self, transaction: Tx, location: Location) {
        if let Some(locations) = self.locations.get_mut(&transaction) {
            locations.remove(&location);
            if locations.is_empty() {
                self.locations.remove(&transaction);
            }
        }
    }

    fn contains(&self, transaction: Tx, location: Location) -> bool {
        self.locations
            .get(&transaction)
            .is_some_and(|locations| locations.contains(&location))
    }

    fn enqueue_remote(&mut self, transaction: Tx, peer: Peer, bytes: u64) {
        self.add_location(transaction, Location::VerifyQueue);
        self.peers.insert(transaction, peer);
        self.bytes.insert(transaction, bytes);
    }

    /// `VerifyQueue::pop_front` removes residency before `_process_tx` starts.
    fn checkout_for_verification(&mut self, transaction: Tx) {
        self.remove_location(transaction, Location::VerifyQueue);
        self.add_location(transaction, Location::ActiveVerifier);
    }

    /// `ban_malformed` can remove only entries still visible in VerifyQueue.
    fn ban_peer(&mut self, peer: Peer) {
        let queued: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(transaction, owner)| {
                (*owner == peer && self.contains(*transaction, Location::VerifyQueue))
                    .then_some(*transaction)
            })
            .collect();
        for transaction in queued {
            self.remove_location(transaction, Location::VerifyQueue);
        }
    }

    fn complete_acceptance(&mut self, transaction: Tx) {
        self.remove_location(transaction, Location::ActiveVerifier);
        self.add_location(transaction, Location::Accepted);
        self.publications
            .insert((transaction, Publication::MembershipCommitted));
    }

    /// Mirrors `process_rbf`: victims leave Accepted and enter the conflict
    /// cache before candidate insertion and capacity retention are known.
    fn begin_rbf(&mut self, victim: Tx) {
        self.remove_location(victim, Location::Accepted);
        self.add_location(victim, Location::ConflictCache);
    }

    fn reject_rbf_candidate(&mut self, candidate: Tx) {
        self.locations.remove(&candidate);
    }

    fn commit_required_effect(&mut self, transaction: Tx) {
        self.publications
            .insert((transaction, Publication::RequiredEffectCommitted));
    }

    fn observe_missing_parent(&mut self, child: Tx, parent: Tx) {
        self.missing_parent.insert(child, parent);
    }

    /// Parent acceptance scans only children already present in OrphanPool.
    fn accept_parent_and_publish_edge(&mut self, parent: Tx) {
        self.add_location(parent, Location::Accepted);
        for (child, observed_parent) in &self.missing_parent {
            if *observed_parent == parent && self.contains(*child, Location::Orphan) {
                self.pending_orphan_wakes.insert(*child);
            }
        }
    }

    fn park_orphan_after_missing_result(&mut self, child: Tx) {
        self.add_location(child, Location::Orphan);
    }

    fn seed_accepted(&mut self, transaction: Tx, bytes: u64) {
        self.add_location(transaction, Location::Accepted);
        self.bytes.insert(transaction, bytes);
    }

    fn seed_gap(&mut self, transaction: Tx) {
        self.add_location(transaction, Location::Accepted);
        self.statuses.insert(transaction, PoolStatus::Gap);
    }

    /// The legacy mine-mode reconciliation has no Gap-to-Pending branch when
    /// the new proposal window contains neither status.
    fn reconcile_gap_outside_window(&mut self, _transaction: Tx) {}

    fn select_detached_uncle_with_proposal(&mut self, transaction: Tx) {
        self.selected_uncle_proposals.insert(transaction);
    }

    fn proposal_eligible(&self, transaction: Tx) -> bool {
        self.statuses.get(&transaction) == Some(&PoolStatus::Pending)
            && !self.selected_uncle_proposals.contains(&transaction)
    }

    fn begin_reorg(&mut self, recovered: Tx) {
        self.detached_recovery.insert(recovered);
    }

    fn clear_pool(&mut self) {
        self.locations.clear();
        self.statuses.clear();
    }

    /// The separately spawned reorg task can acquire the pool lock after a
    /// later ClearPool task and re-add its detached recovery set.
    fn finish_reorg(&mut self) {
        let recovered: Vec<_> = self.detached_recovery.iter().copied().collect();
        for transaction in recovered {
            self.add_location(transaction, Location::Accepted);
        }
        self.detached_recovery.clear();
    }

    fn cache_verified_witness(&mut self, witness: [u8; 4]) {
        self.cache.insert(CacheIdentity::Witness(witness));
    }

    fn cache_lookup_raw(&self, raw: [u8; 4]) -> bool {
        self.cache.contains(&CacheIdentity::Raw(raw))
    }

    fn retained_bytes(&self) -> u64 {
        self.locations
            .keys()
            .filter_map(|transaction| self.bytes.get(transaction))
            .copied()
            .sum()
    }

    fn accepted_bytes(&self) -> u64 {
        self.locations
            .iter()
            .filter_map(|(transaction, locations)| {
                locations
                    .contains(&Location::Accepted)
                    .then(|| self.bytes.get(transaction))
                    .flatten()
            })
            .copied()
            .sum()
    }

    fn verify_queue_bytes(&self) -> u64 {
        self.locations
            .iter()
            .filter_map(|(transaction, locations)| {
                locations
                    .contains(&Location::VerifyQueue)
                    .then(|| self.bytes.get(transaction))
                    .flatten()
            })
            .copied()
            .sum()
    }
}

#[test]
fn develop_peer_ban_can_miss_checked_out_remote_work() {
    let transaction = Tx(1);
    let peer = Peer(7);
    let mut develop = DevelopWitness::default();
    develop.enqueue_remote(transaction, peer, 10);
    develop.checkout_for_verification(transaction);

    develop.ban_peer(peer);
    develop.complete_acceptance(transaction);

    assert!(develop.contains(transaction, Location::Accepted));
}

#[test]
fn develop_failed_rbf_can_remove_the_victim_before_candidate_retention() {
    let victim = Tx(1);
    let candidate = Tx(2);
    let mut develop = DevelopWitness::default();
    develop.seed_accepted(victim, 10);

    develop.begin_rbf(victim);
    develop.reject_rbf_candidate(candidate);

    assert!(!develop.contains(victim, Location::Accepted));
    assert!(develop.contains(victim, Location::ConflictCache));
    assert!(!develop.locations.contains_key(&candidate));
}

#[test]
fn develop_membership_commit_can_be_observed_without_a_required_effect() {
    let transaction = Tx(1);
    let mut develop = DevelopWitness::default();
    develop.enqueue_remote(transaction, Peer(1), 10);
    develop.checkout_for_verification(transaction);
    develop.complete_acceptance(transaction);

    assert!(
        develop
            .publications
            .contains(&(transaction, Publication::MembershipCommitted))
    );
    assert!(
        !develop
            .publications
            .contains(&(transaction, Publication::RequiredEffectCommitted))
    );

    develop.commit_required_effect(transaction);
    assert!(
        develop
            .publications
            .contains(&(transaction, Publication::RequiredEffectCommitted))
    );
}

#[test]
fn develop_fragmented_limits_can_pass_without_one_total_retained_budget() {
    const ACCEPTED_LIMIT: u64 = 180;
    const VERIFY_QUEUE_LIMIT: u64 = 256;

    let mut develop = DevelopWitness::default();
    develop.seed_accepted(Tx(1), ACCEPTED_LIMIT);
    develop.enqueue_remote(Tx(2), Peer(1), VERIFY_QUEUE_LIMIT);

    assert!(develop.accepted_bytes() <= ACCEPTED_LIMIT);
    assert!(develop.verify_queue_bytes() <= VERIFY_QUEUE_LIMIT);
    assert!(develop.retained_bytes() > ACCEPTED_LIMIT);
    assert!(develop.retained_bytes() > VERIFY_QUEUE_LIMIT);
}

#[test]
fn develop_edge_triggered_orphan_wake_can_lose_parent_progress() {
    let parent = Tx(1);
    let child = Tx(2);
    let mut develop = DevelopWitness::default();

    develop.observe_missing_parent(child, parent);
    develop.accept_parent_and_publish_edge(parent);
    develop.park_orphan_after_missing_result(child);

    assert!(develop.contains(child, Location::Orphan));
    assert!(!develop.pending_orphan_wakes.contains(&child));
}

#[test]
fn develop_gap_and_detached_uncle_can_jointly_censor_reproposal() {
    let transaction = Tx(1);
    let mut develop = DevelopWitness::default();
    develop.seed_gap(transaction);
    develop.reconcile_gap_outside_window(transaction);
    develop.select_detached_uncle_with_proposal(transaction);

    assert_eq!(develop.statuses.get(&transaction), Some(&PoolStatus::Gap));
    assert!(!develop.proposal_eligible(transaction));
}

#[test]
fn develop_concurrent_clear_can_be_overtaken_by_detached_recovery() {
    let recovered = Tx(1);
    let mut develop = DevelopWitness::default();
    develop.begin_reorg(recovered);
    develop.clear_pool();
    develop.finish_reorg();

    assert!(develop.contains(recovered, Location::Accepted));
}

#[test]
fn develop_detached_cache_lookup_can_substitute_raw_for_witness_identity() {
    let mut develop = DevelopWitness::default();
    develop.cache_verified_witness([1, 2, 3, 4]);

    assert!(!develop.cache_lookup_raw([9, 8, 7, 6]));
}
