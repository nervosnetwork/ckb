//! Authoritative pre-pool transaction-lifecycle ownership.
//!
//! This module is intentionally introduced before it is wired into the hot
//! path.  It defines the state and atomicity contract that the coordinator and
//! commit sequencer will use while the legacy queues are replaced
//! incrementally.  Until that migration is complete, its model tests are the
//! executable specification for the target architecture.
#![allow(dead_code)]

use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Pipeline stage that owns a queued or active unit of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PipelineStage {
    PreCheck,
    Resolve,
    Verify,
}

/// The one authoritative location of a transaction before pool acceptance.
/// A successful commit terminalizes this record and transfers ownership to
/// `TxPool`; accepted proposal-window state is never mirrored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleLocation {
    Queued(PipelineStage),
    Active(PipelineStage),
    WaitingParents,
    WaitingInputs,
    WaitingConflict { winner: Byte32 },
    ReadyToCommit,
    Committing,
}

/// A normalized location used by indexes and accounting.  Conflict winners
/// are deliberately not part of this key; the conflict scheduler owns that
/// ID-only relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LifecycleLocationKind {
    QueuedPreCheck,
    ActivePreCheck,
    QueuedResolve,
    ActiveResolve,
    QueuedVerify,
    ActiveVerify,
    WaitingParents,
    WaitingInputs,
    WaitingConflict,
    ReadyToCommit,
    Committing,
}

impl LifecycleLocation {
    pub(crate) fn kind(&self) -> LifecycleLocationKind {
        match self {
            Self::Queued(PipelineStage::PreCheck) => LifecycleLocationKind::QueuedPreCheck,
            Self::Active(PipelineStage::PreCheck) => LifecycleLocationKind::ActivePreCheck,
            Self::Queued(PipelineStage::Resolve) => LifecycleLocationKind::QueuedResolve,
            Self::Active(PipelineStage::Resolve) => LifecycleLocationKind::ActiveResolve,
            Self::Queued(PipelineStage::Verify) => LifecycleLocationKind::QueuedVerify,
            Self::Active(PipelineStage::Verify) => LifecycleLocationKind::ActiveVerify,
            Self::WaitingParents => LifecycleLocationKind::WaitingParents,
            Self::WaitingInputs => LifecycleLocationKind::WaitingInputs,
            Self::WaitingConflict { .. } => LifecycleLocationKind::WaitingConflict,
            Self::ReadyToCommit => LifecycleLocationKind::ReadyToCommit,
            Self::Committing => LifecycleLocationKind::Committing,
        }
    }
}

/// Count/byte usage or limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Residency {
    pub(crate) entries: usize,
    pub(crate) bytes: usize,
}

impl Residency {
    pub(crate) const fn new(entries: usize, bytes: usize) -> Self {
        Self { entries, bytes }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries && self.bytes <= limit.bytes
    }
}

/// Continuous lifecycle limits.  Local/proposal transactions consume only the
/// global limit; remote transactions also consume the per-peer limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LifecycleLimits {
    pub(crate) global: Residency,
    pub(crate) per_peer: Option<Residency>,
}

impl LifecycleLimits {
    pub(crate) const fn new(global: Residency, per_peer: Option<Residency>) -> Self {
        Self { global, per_peer }
    }
}

/// Incarnation and revision used for compare-and-swap transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LifecycleVersion {
    pub(crate) incarnation: u64,
    pub(crate) revision: u64,
}

/// Immutable view returned to readers.  Payload access is separate and returns
/// an `Arc`, so indexes and readers never become competing payload owners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleView {
    pub(crate) short_id: ProposalShortId,
    pub(crate) location: LifecycleLocation,
    pub(crate) peer: Option<PeerIndex>,
    pub(crate) charge_bytes: usize,
    pub(crate) version: LifecycleVersion,
}

#[derive(Debug)]
struct LifecycleEntry<P> {
    short_id: ProposalShortId,
    payload: Arc<P>,
    location: LifecycleLocation,
    peer: Option<PeerIndex>,
    charge_bytes: usize,
    incarnation: u64,
    revision: u64,
}

impl<P> LifecycleEntry<P> {
    fn version(&self) -> LifecycleVersion {
        LifecycleVersion {
            incarnation: self.incarnation,
            revision: self.revision,
        }
    }

    fn view(&self) -> LifecycleView {
        LifecycleView {
            short_id: self.short_id.clone(),
            location: self.location.clone(),
            peer: self.peer,
            charge_bytes: self.charge_bytes,
            version: self.version(),
        }
    }
}

/// A worker's immutable payload plus a versioned right to complete one active
/// stage.  Removing and re-admitting the same hash creates a new incarnation,
/// so a stale worker can never complete the new transaction record.
#[derive(Debug, Clone)]
pub(crate) struct WorkLease<P> {
    pub(crate) hash: Byte32,
    pub(crate) stage: PipelineStage,
    pub(crate) version: LifecycleVersion,
    pub(crate) payload: Arc<P>,
}

/// One member of an all-or-nothing lifecycle transition batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleTransition {
    pub(crate) hash: Byte32,
    pub(crate) version: LifecycleVersion,
    pub(crate) expected: LifecycleLocation,
    pub(crate) next: LifecycleLocation,
}

/// One operation in an atomic ownership commit batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleBatchOp {
    Transition(LifecycleTransition),
    Terminalize {
        hash: Byte32,
        version: LifecycleVersion,
        expected: LifecycleLocation,
        outcome: TerminalOutcome,
    },
}

#[derive(Debug)]
pub(crate) enum LifecycleBatchResult<P> {
    Transitioned {
        hash: Byte32,
        version: LifecycleVersion,
    },
    Terminalized(TerminalEntry<P>),
}

/// Why a payload left the live lifecycle store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalOutcome {
    Committed,
    Rejected,
    Removed,
    Cleared,
}

/// Payload returned to the coordinator after terminalization.  External side
/// effects are emitted only after this value has left the store lock.
#[derive(Debug)]
pub(crate) struct TerminalEntry<P> {
    pub(crate) hash: Byte32,
    pub(crate) short_id: ProposalShortId,
    pub(crate) payload: Arc<P>,
    pub(crate) peer: Option<PeerIndex>,
    pub(crate) outcome: TerminalOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleError {
    DuplicateHash(Byte32),
    ShortIdCollision {
        short_id: ProposalShortId,
        existing_hash: Byte32,
    },
    InvalidInitialLocation(LifecycleLocation),
    GlobalBudgetExceeded,
    PeerBudgetExceeded(PeerIndex),
    IncarnationExhausted,
    Missing(Byte32),
    IncarnationMismatch {
        expected: u64,
        actual: u64,
    },
    RevisionMismatch {
        expected: u64,
        actual: u64,
    },
    LocationMismatch {
        expected: LifecycleLocation,
        actual: LifecycleLocation,
    },
    IllegalTransition {
        from: LifecycleLocation,
        to: LifecycleLocation,
    },
    DuplicateBatchEntry(Byte32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleAuditError {
    GlobalUsage {
        expected: Residency,
        actual: Residency,
    },
    PeerUsage,
    ShortIdIndex,
    PeerIndex,
    LocationIndex,
}

/// Single payload owner plus ID-only indexes.
#[derive(Debug)]
pub(crate) struct LifecycleStore<P> {
    entries: HashMap<Byte32, LifecycleEntry<P>>,
    by_short_id: HashMap<ProposalShortId, Byte32>,
    by_peer: HashMap<PeerIndex, HashSet<Byte32>>,
    by_location: HashMap<LifecycleLocationKind, HashSet<Byte32>>,
    global_usage: Residency,
    peer_usage: HashMap<PeerIndex, Residency>,
    limits: LifecycleLimits,
    next_incarnation: u64,
}

impl<P> LifecycleStore<P> {
    pub(crate) fn new(limits: LifecycleLimits) -> Self {
        Self {
            entries: HashMap::new(),
            by_short_id: HashMap::new(),
            by_peer: HashMap::new(),
            by_location: HashMap::new(),
            global_usage: Residency::default(),
            peer_usage: HashMap::new(),
            limits,
            next_incarnation: 1,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn usage(&self) -> Residency {
        self.global_usage
    }

    pub(crate) fn peer_usage(&self, peer: PeerIndex) -> Residency {
        self.peer_usage.get(&peer).copied().unwrap_or_default()
    }

    pub(crate) fn location_len(&self, kind: LifecycleLocationKind) -> usize {
        self.by_location.get(&kind).map_or(0, HashSet::len)
    }

    pub(crate) fn view(&self, hash: &Byte32) -> Option<LifecycleView> {
        self.entries.get(hash).map(LifecycleEntry::view)
    }

    pub(crate) fn payload(&self, hash: &Byte32) -> Option<Arc<P>> {
        self.entries
            .get(hash)
            .map(|entry| Arc::clone(&entry.payload))
    }

    pub(crate) fn hash_by_short_id(&self, short_id: &ProposalShortId) -> Option<&Byte32> {
        self.by_short_id.get(short_id)
    }

    pub(crate) fn admit(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        payload: P,
        location: LifecycleLocation,
        peer: Option<PeerIndex>,
        charge_bytes: usize,
    ) -> Result<LifecycleVersion, LifecycleError> {
        if self.entries.contains_key(&hash) {
            return Err(LifecycleError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(LifecycleError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        if !Self::is_initial_location(&location) {
            return Err(LifecycleError::InvalidInitialLocation(location));
        }

        let charge = Residency::new(1, charge_bytes);
        self.check_add_budget(peer, charge)?;
        let incarnation = self.next_incarnation;
        self.next_incarnation = self
            .next_incarnation
            .checked_add(1)
            .ok_or(LifecycleError::IncarnationExhausted)?;

        let entry = LifecycleEntry {
            short_id: short_id.clone(),
            payload: Arc::new(payload),
            location: location.clone(),
            peer,
            charge_bytes,
            incarnation,
            revision: 0,
        };
        self.global_usage = self
            .global_usage
            .checked_add(charge)
            .expect("budget check guarantees global usage");
        if let Some(peer) = peer {
            let usage = self.peer_usage.entry(peer).or_default();
            *usage = usage
                .checked_add(charge)
                .expect("budget check guarantees peer usage");
            self.by_peer.entry(peer).or_default().insert(hash.clone());
        }
        self.by_short_id.insert(short_id, hash.clone());
        self.by_location
            .entry(location.kind())
            .or_default()
            .insert(hash.clone());
        self.entries.insert(hash, entry);

        Ok(LifecycleVersion {
            incarnation,
            revision: 0,
        })
    }

    /// Move one queued record into an active stage and return an immutable
    /// worker lease.
    pub(crate) fn checkout(
        &mut self,
        hash: &Byte32,
        stage: PipelineStage,
    ) -> Result<WorkLease<P>, LifecycleError> {
        let view = self
            .view(hash)
            .ok_or_else(|| LifecycleError::Missing(hash.clone()))?;
        let expected = LifecycleLocation::Queued(stage);
        let next = LifecycleLocation::Active(stage);
        let version = self.transition(hash, view.version, &expected, next)?;
        let payload = self
            .payload(hash)
            .expect("transitioned entry remains present");
        Ok(WorkLease {
            hash: hash.clone(),
            stage,
            version,
            payload,
        })
    }

    /// Complete a worker lease, optionally replacing the immutable payload and
    /// its continuous residency charge (for example raw -> resolved).
    pub(crate) fn complete(
        &mut self,
        lease: &WorkLease<P>,
        next: LifecycleLocation,
        replacement: Option<(P, usize)>,
    ) -> Result<LifecycleVersion, LifecycleError> {
        let expected = LifecycleLocation::Active(lease.stage);
        self.validate_transition(&lease.hash, lease.version, &expected, &next)?;

        if let Some((_, new_charge_bytes)) = &replacement {
            self.check_recharge(&lease.hash, *new_charge_bytes)?;
        }
        self.apply_location_change(&lease.hash, next);
        if let Some((payload, new_charge_bytes)) = replacement {
            self.apply_recharge(&lease.hash, new_charge_bytes);
            self.entries
                .get_mut(&lease.hash)
                .expect("validated entry")
                .payload = Arc::new(payload);
        }
        Ok(self
            .entries
            .get(&lease.hash)
            .expect("completed entry remains present")
            .version())
    }

    /// Compare-and-swap one lifecycle location.
    pub(crate) fn transition(
        &mut self,
        hash: &Byte32,
        version: LifecycleVersion,
        expected: &LifecycleLocation,
        next: LifecycleLocation,
    ) -> Result<LifecycleVersion, LifecycleError> {
        self.validate_transition(hash, version, expected, &next)?;
        self.apply_location_change(hash, next);
        Ok(self
            .entries
            .get(hash)
            .expect("transitioned entry remains present")
            .version())
    }

    /// Atomically apply a batch of existing-entry transitions.  Validation of
    /// every member (including duplicate-ID rejection) completes before the
    /// first index or entry is mutated.
    pub(crate) fn transition_batch(
        &mut self,
        transitions: &[LifecycleTransition],
    ) -> Result<Vec<LifecycleVersion>, LifecycleError> {
        let mut unique = HashSet::with_capacity(transitions.len());
        for transition in transitions {
            if !unique.insert(transition.hash.clone()) {
                return Err(LifecycleError::DuplicateBatchEntry(transition.hash.clone()));
            }
            self.validate_transition(
                &transition.hash,
                transition.version,
                &transition.expected,
                &transition.next,
            )?;
        }

        let mut versions = Vec::with_capacity(transitions.len());
        for transition in transitions {
            self.apply_location_change(&transition.hash, transition.next.clone());
            versions.push(
                self.entries
                    .get(&transition.hash)
                    .expect("validated batch entry remains present")
                    .version(),
            );
        }
        Ok(versions)
    }

    /// Apply location moves and terminal removals as one all-or-nothing
    /// lifecycle operation. This is the ownership-side commit primitive for
    /// an RBF swap: the winner is terminalized as handed to `TxPool` in the
    /// same batch that speculative victims are terminalized as rejected.
    pub(crate) fn apply_batch(
        &mut self,
        operations: &[LifecycleBatchOp],
    ) -> Result<Vec<LifecycleBatchResult<P>>, LifecycleError> {
        let mut unique = HashSet::with_capacity(operations.len());
        for operation in operations {
            let hash = match operation {
                LifecycleBatchOp::Transition(transition) => {
                    self.validate_transition(
                        &transition.hash,
                        transition.version,
                        &transition.expected,
                        &transition.next,
                    )?;
                    &transition.hash
                }
                LifecycleBatchOp::Terminalize {
                    hash,
                    version,
                    expected,
                    ..
                } => {
                    self.validate_version_and_location(hash, *version, expected)?;
                    hash
                }
            };
            if !unique.insert(hash.clone()) {
                return Err(LifecycleError::DuplicateBatchEntry(hash.clone()));
            }
        }

        let mut results = Vec::with_capacity(operations.len());
        for operation in operations {
            match operation {
                LifecycleBatchOp::Transition(transition) => {
                    self.apply_location_change(&transition.hash, transition.next.clone());
                    results.push(LifecycleBatchResult::Transitioned {
                        hash: transition.hash.clone(),
                        version: self
                            .entries
                            .get(&transition.hash)
                            .expect("validated batch entry remains present")
                            .version(),
                    });
                }
                LifecycleBatchOp::Terminalize { hash, outcome, .. } => {
                    results.push(LifecycleBatchResult::Terminalized(
                        self.remove_present(hash, *outcome),
                    ));
                }
            }
        }
        Ok(results)
    }

    /// Remove an entry after a compare-and-swap check.  The returned payload is
    /// the stable-state boundary for callbacks, relay, metrics and persistence.
    pub(crate) fn terminalize(
        &mut self,
        hash: &Byte32,
        version: LifecycleVersion,
        expected: &LifecycleLocation,
        outcome: TerminalOutcome,
    ) -> Result<TerminalEntry<P>, LifecycleError> {
        self.validate_version_and_location(hash, version, expected)?;
        Ok(self.remove_present(hash, outcome))
    }

    /// Administrative removal deliberately does not require a worker version.
    /// A stale lease is still safe because a later re-admission gets a fresh
    /// incarnation.
    pub(crate) fn force_remove(
        &mut self,
        hash: &Byte32,
        outcome: TerminalOutcome,
    ) -> Option<TerminalEntry<P>> {
        self.entries
            .contains_key(hash)
            .then(|| self.remove_present(hash, outcome))
    }

    pub(crate) fn clear(&mut self) -> Vec<TerminalEntry<P>> {
        let hashes: Vec<_> = self.entries.keys().cloned().collect();
        hashes
            .iter()
            .map(|hash| self.remove_present(hash, TerminalOutcome::Cleared))
            .collect()
    }

    /// Recompute every index and counter from the authoritative entries.
    pub(crate) fn audit(&self) -> Result<(), LifecycleAuditError> {
        let mut global_usage = Residency::default();
        let mut peer_usage: HashMap<PeerIndex, Residency> = HashMap::new();
        let mut by_short_id = HashMap::new();
        let mut by_peer: HashMap<PeerIndex, HashSet<Byte32>> = HashMap::new();
        let mut by_location: HashMap<LifecycleLocationKind, HashSet<Byte32>> = HashMap::new();

        for (hash, entry) in &self.entries {
            let charge = Residency::new(1, entry.charge_bytes);
            global_usage =
                global_usage
                    .checked_add(charge)
                    .ok_or(LifecycleAuditError::GlobalUsage {
                        expected: self.global_usage,
                        actual: Residency::new(usize::MAX, usize::MAX),
                    })?;
            by_short_id.insert(entry.short_id.clone(), hash.clone());
            by_location
                .entry(entry.location.kind())
                .or_default()
                .insert(hash.clone());
            if let Some(peer) = entry.peer {
                let usage = peer_usage.entry(peer).or_default();
                *usage = usage
                    .checked_add(charge)
                    .ok_or(LifecycleAuditError::PeerUsage)?;
                by_peer.entry(peer).or_default().insert(hash.clone());
            }
        }

        if global_usage != self.global_usage {
            return Err(LifecycleAuditError::GlobalUsage {
                expected: global_usage,
                actual: self.global_usage,
            });
        }
        if peer_usage != self.peer_usage {
            return Err(LifecycleAuditError::PeerUsage);
        }
        if by_short_id != self.by_short_id {
            return Err(LifecycleAuditError::ShortIdIndex);
        }
        if by_peer != self.by_peer {
            return Err(LifecycleAuditError::PeerIndex);
        }
        if by_location != self.by_location {
            return Err(LifecycleAuditError::LocationIndex);
        }
        Ok(())
    }

    fn validate_transition(
        &self,
        hash: &Byte32,
        version: LifecycleVersion,
        expected: &LifecycleLocation,
        next: &LifecycleLocation,
    ) -> Result<(), LifecycleError> {
        self.validate_version_and_location(hash, version, expected)?;
        if !Self::is_legal_transition(expected, next) {
            return Err(LifecycleError::IllegalTransition {
                from: expected.clone(),
                to: next.clone(),
            });
        }
        Ok(())
    }

    fn validate_version_and_location(
        &self,
        hash: &Byte32,
        version: LifecycleVersion,
        expected: &LifecycleLocation,
    ) -> Result<(), LifecycleError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| LifecycleError::Missing(hash.clone()))?;
        if entry.incarnation != version.incarnation {
            return Err(LifecycleError::IncarnationMismatch {
                expected: version.incarnation,
                actual: entry.incarnation,
            });
        }
        if entry.revision != version.revision {
            return Err(LifecycleError::RevisionMismatch {
                expected: version.revision,
                actual: entry.revision,
            });
        }
        if entry.location != *expected {
            return Err(LifecycleError::LocationMismatch {
                expected: expected.clone(),
                actual: entry.location.clone(),
            });
        }
        Ok(())
    }

    fn apply_location_change(&mut self, hash: &Byte32, next: LifecycleLocation) {
        let old_kind = self
            .entries
            .get(hash)
            .expect("validated lifecycle entry")
            .location
            .kind();
        if let Some(ids) = self.by_location.get_mut(&old_kind) {
            ids.remove(hash);
            if ids.is_empty() {
                self.by_location.remove(&old_kind);
            }
        }
        self.by_location
            .entry(next.kind())
            .or_default()
            .insert(hash.clone());
        let entry = self
            .entries
            .get_mut(hash)
            .expect("validated lifecycle entry");
        entry.location = next;
        entry.revision = entry
            .revision
            .checked_add(1)
            .expect("lifecycle revision space exhausted");
    }

    fn check_add_budget(
        &self,
        peer: Option<PeerIndex>,
        charge: Residency,
    ) -> Result<(), LifecycleError> {
        let next_global = self
            .global_usage
            .checked_add(charge)
            .ok_or(LifecycleError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(LifecycleError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (peer, self.limits.per_peer) {
            let next_peer = self
                .peer_usage(peer)
                .checked_add(charge)
                .ok_or(LifecycleError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(LifecycleError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    fn check_recharge(&self, hash: &Byte32, new_charge_bytes: usize) -> Result<(), LifecycleError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| LifecycleError::Missing(hash.clone()))?;
        let old_charge = Residency::new(1, entry.charge_bytes);
        let new_charge = Residency::new(1, new_charge_bytes);
        let next_global = self
            .global_usage
            .checked_sub(old_charge)
            .and_then(|usage| usage.checked_add(new_charge))
            .ok_or(LifecycleError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(LifecycleError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (entry.peer, self.limits.per_peer) {
            let next_peer = self
                .peer_usage(peer)
                .checked_sub(old_charge)
                .and_then(|usage| usage.checked_add(new_charge))
                .ok_or(LifecycleError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(LifecycleError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    fn apply_recharge(&mut self, hash: &Byte32, new_charge_bytes: usize) {
        let (peer, old_charge_bytes) = {
            let entry = self.entries.get(hash).expect("validated lifecycle entry");
            (entry.peer, entry.charge_bytes)
        };
        let old_charge = Residency::new(1, old_charge_bytes);
        let new_charge = Residency::new(1, new_charge_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(old_charge)
            .and_then(|usage| usage.checked_add(new_charge))
            .expect("recharge was validated");
        if let Some(peer) = peer {
            let usage = self.peer_usage.get_mut(&peer).expect("peer usage exists");
            *usage = usage
                .checked_sub(old_charge)
                .and_then(|usage| usage.checked_add(new_charge))
                .expect("peer recharge was validated");
        }
        self.entries
            .get_mut(hash)
            .expect("validated lifecycle entry")
            .charge_bytes = new_charge_bytes;
    }

    fn remove_present(&mut self, hash: &Byte32, outcome: TerminalOutcome) -> TerminalEntry<P> {
        let entry = self.entries.remove(hash).expect("lifecycle entry present");
        let charge = Residency::new(1, entry.charge_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(charge)
            .expect("authoritative global accounting");
        self.by_short_id.remove(&entry.short_id);

        if let Some(ids) = self.by_location.get_mut(&entry.location.kind()) {
            ids.remove(hash);
            if ids.is_empty() {
                self.by_location.remove(&entry.location.kind());
            }
        }
        if let Some(peer) = entry.peer {
            let remove_peer_usage = {
                let usage = self.peer_usage.get_mut(&peer).expect("peer usage exists");
                *usage = usage
                    .checked_sub(charge)
                    .expect("authoritative peer accounting");
                *usage == Residency::default()
            };
            if remove_peer_usage {
                self.peer_usage.remove(&peer);
            }
            if let Some(ids) = self.by_peer.get_mut(&peer) {
                ids.remove(hash);
                if ids.is_empty() {
                    self.by_peer.remove(&peer);
                }
            }
        }

        TerminalEntry {
            hash: hash.clone(),
            short_id: entry.short_id,
            payload: entry.payload,
            peer: entry.peer,
            outcome,
        }
    }

    fn is_initial_location(location: &LifecycleLocation) -> bool {
        matches!(
            location,
            LifecycleLocation::Queued(_)
                | LifecycleLocation::WaitingParents
                | LifecycleLocation::WaitingInputs
        )
    }

    fn is_legal_transition(from: &LifecycleLocation, to: &LifecycleLocation) -> bool {
        use LifecycleLocation as L;
        use PipelineStage as S;

        match (from, to) {
            (L::Queued(left), L::Active(right)) => left == right,
            // Panic/cancellation recovery returns the same stage to its queue.
            (L::Active(left), L::Queued(right)) if left == right => true,
            (L::Active(S::PreCheck), L::Queued(S::Resolve)) => true,
            (L::Active(S::PreCheck | S::Resolve), L::WaitingParents) => true,
            (L::Active(S::Resolve | S::Verify), L::WaitingInputs) => true,
            (L::Active(S::Resolve), L::Queued(S::Verify)) => true,
            (L::Active(S::Verify), L::WaitingConflict { .. } | L::ReadyToCommit) => true,
            (L::WaitingParents, L::Queued(S::Resolve)) => true,
            (L::WaitingInputs, L::Queued(S::Resolve | S::Verify)) => true,
            (L::WaitingConflict { .. }, L::Queued(S::Verify)) => true,
            (L::ReadyToCommit, L::Committing) => true,
            (L::Committing, L::Queued(S::Verify)) => true,
            _ => false,
        }
    }
}
