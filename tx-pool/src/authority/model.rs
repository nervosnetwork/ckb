use super::store::ReadSet;
use crate::component::entry::TxEntry;
use crate::error::Reject;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, Cycle, TransactionView, cell::ResolvedTransaction},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::{
    collections::BTreeSet,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

/// Resource failures that select distinct retry policies have named reasons.
/// Other limits carry diagnostic text; wording never selects control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FullReason {
    Accepted,
    Pipeline,
    History,
    NoticeOutbox,
    ChainTransition,
    Other(&'static str),
}
impl From<&'static str> for FullReason {
    fn from(reason: &'static str) -> Self {
        Self::Other(reason)
    }
}
impl fmt::Display for FullReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Accepted => "accepted pool",
            Self::Pipeline => "pipeline",
            Self::History => "replacement history",
            Self::NoticeOutbox => "notice outbox",
            Self::ChainTransition => "chain transition",
            Self::Other(reason) => reason,
        })
    }
}

/// Expected operation outcomes share one route through the store/service.
/// A structural fault is distinct from input, stale work and resource pressure.
#[derive(Debug, Clone)]
pub(crate) enum Error {
    Stale,
    Full(FullReason),
    Rejected(Reject),
    Closed,
    Fault(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale => f.write_str("tx-pool observation changed"),
            Self::Full(reason) => write!(f, "tx-pool resource unavailable: {reason}"),
            Self::Rejected(reason) => fmt::Display::fmt(reason, f),
            Self::Closed => f.write_str("tx-pool is closed"),
            Self::Fault(reason) => write!(f, "tx-pool invariant failed: {reason}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<Reject> for Error {
    fn from(value: Reject) -> Self {
        Self::Rejected(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Remote {
        peer: PeerIndex,
        deadline: Instant,
        cycles: Option<Cycle>,
    },
    Proposal {
        remote: Option<(PeerIndex, Instant)>,
    },
    Recovery,
    Local,
}

impl Source {
    pub(super) fn residency_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote { peer, .. } => Some(peer),
            Self::Proposal { remote } => remote.map(|(peer, _)| peer),
            Self::Recovery | Self::Local => None,
        }
    }
    pub(super) fn compute_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote { peer, .. } => Some(peer),
            _ => None,
        }
    }
    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            Self::Remote { deadline, .. } => Some(deadline),
            Self::Proposal { remote } => remote.map(|(_, deadline)| deadline),
            Self::Recovery | Self::Local => None,
        }
    }
    pub(super) fn declared_cycles(self) -> Option<Cycle> {
        match self {
            Self::Remote { cycles, .. } => cycles,
            _ => None,
        }
    }
    /// Require a known pending producer before waiting for a missing cell.
    pub(super) fn requires_known_producer(self) -> bool {
        !matches!(
            self,
            Self::Remote {
                cycles: Some(_),
                ..
            }
        )
    }
    pub(super) fn priority(self) -> u8 {
        match self {
            Self::Recovery | Self::Local => 0,
            Self::Proposal { .. } => 1,
            Self::Remote { .. } => 2,
        }
    }
    pub(super) fn duration(self, config: &ckb_app_config::TxPoolConfig) -> Duration {
        let cap = u64::from(config.max_tx_verify_time_ms);
        let millis = self.declared_cycles().map_or(cap, |cycles| {
            cycles
                .div_ceil(config.tx_verify_cycles_per_ms.max(1))
                .max(u64::from(config.min_tx_verify_time_ms))
                .min(cap)
        });
        Duration::from_millis(millis)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum DependencyKey {
    Cell(OutPoint),
    Header(Byte32),
}

fn sorted_dependencies(keys: impl Iterator<Item = DependencyKey>) -> Box<[DependencyKey]> {
    let mut keys: Vec<_> = keys.collect();
    keys.sort_unstable();
    keys.dedup();
    // Do not retain raw-duplicate capacity in the temporary read-only result.
    keys.into_boxed_slice()
}

/// Relations have one source: an owner's input/dep/parent/waiting fields.
/// Children uses shared gates for point additions, preserving sibling overlap.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum RelationKey {
    Dependency(DependencyKey),
    Children(Byte32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Status {
    Pending,
    Gap,
    Proposed,
}

pub(super) fn status(snapshot: &Snapshot, proposal: &ProposalShortId) -> Status {
    if snapshot.proposals().contains_proposed(proposal) {
        Status::Proposed
    } else if snapshot.proposals().contains_gap(proposal) {
        Status::Gap
    } else {
        Status::Pending
    }
}

/// Resolution retains compact cell content and weak producer observations.
/// The snapshot is held only by bounded active work, never by queued entries.
#[derive(Clone, Debug)]
pub(super) struct Resolved {
    pub(super) transaction: Arc<ResolvedTransaction>,
    pub(super) fee: Capacity,
    pub(super) view: u64,
    pub(super) reads: ReadSet,
    pub(super) pool_cells: BTreeSet<OutPoint>,
}

#[derive(Clone, Debug)]
pub(super) struct Accepted {
    pub(super) transaction: Arc<ResolvedTransaction>,
    pub(super) cycles: Cycle,
    pub(super) fee: Capacity,
    pub(super) size: usize,
    pub(super) timestamp: u64,
    pub(super) parents: BTreeSet<Byte32>,
    pub(super) context_sensitive: bool,
    #[cfg(any(test, feature = "internal"))]
    pub(super) forced_status: Option<Status>,
}

impl Accepted {
    pub(super) fn projection(&self) -> TxEntry {
        TxEntry::new_with_timestamp(
            Arc::clone(&self.transaction),
            self.cycles,
            self.fee,
            self.size,
            self.timestamp,
        )
    }
    pub(super) fn dependencies(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.transaction.related_dep_out_points().cloned()
    }
    pub(super) fn status(&self, snapshot: &Snapshot) -> Status {
        #[cfg(any(test, feature = "internal"))]
        if let Some(status) = self.forced_status {
            return status;
        }
        status(snapshot, &self.transaction.transaction.proposal_short_id())
    }
}

#[derive(Clone, Debug)]
pub(super) enum Phase {
    Resolve,
    Verify(Arc<Resolved>),
    Waiting(BTreeSet<DependencyKey>),
    Accepted(Accepted),
    Replaced {
        triggers: BTreeSet<DependencyKey>,
        require_all: bool,
    },
}

/// Values never mutate after insertion; every change constructs a fresh Arc.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub(super) transaction: Arc<TransactionView>,
    pub(super) arrival: u64,
    pub(super) source: Source,
    pub(super) phase: Phase,
}

impl Entry {
    pub(super) fn hash(&self) -> Byte32 {
        self.transaction.hash()
    }
    pub(super) fn proposal(&self) -> ProposalShortId {
        self.transaction.proposal_short_id()
    }
    pub(super) fn accepted(&self) -> Option<&Accepted> {
        match &self.phase {
            Phase::Accepted(entry) => Some(entry),
            _ => None,
        }
    }
    pub(super) fn preaccepted(&self) -> bool {
        !matches!(self.phase, Phase::Accepted(_) | Phase::Replaced { .. })
    }
    pub(super) fn with_phase(&self, phase: Phase) -> Arc<Self> {
        Arc::new(Self {
            transaction: Arc::clone(&self.transaction),
            arrival: self.arrival,
            source: self.source,
            phase,
        })
    }
    pub(super) fn declared_dependencies(&self) -> Box<[DependencyKey]> {
        sorted_dependencies(
            self.transaction
                .input_pts_iter()
                .map(DependencyKey::Cell)
                .chain(
                    self.transaction
                        .cell_deps_iter()
                        .map(|dep| DependencyKey::Cell(dep.out_point())),
                )
                .chain(
                    self.transaction
                        .header_deps_iter()
                        .map(DependencyKey::Header),
                ),
        )
    }
    pub(super) fn dependencies(&self) -> Box<[DependencyKey]> {
        match &self.phase {
            Phase::Accepted(entry) => sorted_dependencies(
                self.transaction
                    .input_pts_iter()
                    .map(DependencyKey::Cell)
                    .chain(entry.dependencies().map(DependencyKey::Cell))
                    .chain(
                        self.transaction
                            .header_deps_iter()
                            .map(DependencyKey::Header),
                    ),
            ),
            Phase::Verify(resolved) => sorted_dependencies(
                resolved
                    .transaction
                    .resolved_inputs
                    .iter()
                    .chain(&resolved.transaction.resolved_cell_deps)
                    .chain(&resolved.transaction.resolved_dep_groups)
                    .map(|cell| DependencyKey::Cell(cell.out_point.clone()))
                    .chain(
                        self.transaction
                            .header_deps_iter()
                            .map(DependencyKey::Header),
                    ),
            ),
            Phase::Waiting(keys) | Phase::Replaced { triggers: keys, .. } => {
                keys.iter().cloned().collect()
            }
            Phase::Resolve => self.declared_dependencies(),
        }
    }
}
