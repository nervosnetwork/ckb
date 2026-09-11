//! Checked owner charges and cancellable positive reservations.
//!
//! Usage is live owner charge plus outstanding positive reservations. The
//! exact old-owner check in Store::apply prevents double borrowing victims.
//! Owner reservations exist only inside the validated owner cut. Active work
//! owns a fixed envelope through a separate count and the running computation.
use super::model::{Entry, Error, FullReason, Phase, Source};
use super::residency::{accepted_transaction_charge_bytes, resolved_transaction_charge_bytes};
use crate::constants::ResidencyLimits;
use ckb_app_config::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_network::PeerIndex;
use ckb_util::parking_lot::Mutex;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

const ENTRY_BYTES: usize = 768 + 160 * 5;
const EDGE_BYTES: usize = 160 * 7;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Amount {
    pub(super) items: usize,
    pub(super) bytes: usize,
    pub(super) edges: usize,
    pub(super) serialized: usize,
    pub(super) cycles: u64,
}

impl Amount {
    pub(super) fn checked_add(self, rhs: Self) -> Option<Self> {
        Some(Self {
            items: self.items.checked_add(rhs.items)?,
            bytes: self.bytes.checked_add(rhs.bytes)?,
            edges: self.edges.checked_add(rhs.edges)?,
            serialized: self.serialized.checked_add(rhs.serialized)?,
            cycles: self.cycles.checked_add(rhs.cycles)?,
        })
    }
    pub(super) fn checked_sub(self, rhs: Self) -> Option<Self> {
        Some(Self {
            items: self.items.checked_sub(rhs.items)?,
            bytes: self.bytes.checked_sub(rhs.bytes)?,
            edges: self.edges.checked_sub(rhs.edges)?,
            serialized: self.serialized.checked_sub(rhs.serialized)?,
            cycles: self.cycles.checked_sub(rhs.cycles)?,
        })
    }
    fn positive_difference(self, rhs: Self) -> Self {
        Self {
            items: self.items.saturating_sub(rhs.items),
            bytes: self.bytes.saturating_sub(rhs.bytes),
            edges: self.edges.saturating_sub(rhs.edges),
            serialized: self.serialized.saturating_sub(rhs.serialized),
            cycles: self.cycles.saturating_sub(rhs.cycles),
        }
    }
    pub(super) fn fits(self, limit: Self) -> bool {
        self.items <= limit.items
            && self.bytes <= limit.bytes
            && self.edges <= limit.edges
            && self.serialized <= limit.serialized
            && self.cycles <= limit.cycles
    }
    fn fraction(self, numerator: usize, denominator: usize) -> Option<Self> {
        Some(Self {
            items: self
                .items
                .checked_mul(numerator)?
                .checked_div(denominator)?,
            bytes: self
                .bytes
                .checked_mul(numerator)?
                .checked_div(denominator)?,
            edges: self
                .edges
                .checked_mul(numerator)?
                .checked_div(denominator)?,
            serialized: self.serialized,
            cycles: self.cycles,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Account {
    Accepted,
    Pipeline,
    Remote,
    Peer(PeerIndex),
    History,
}

#[derive(Clone, Debug)]
pub(super) struct MaterializationLimits {
    pub(super) bytes: usize,
    pub(super) edges: usize,
}

/// Limits are derived once from existing configuration and canonical consensus.
#[derive(Clone, Debug)]
pub(super) struct Limits {
    pub(super) accepted: Amount,
    pub(super) pipeline: Amount,
    remote: Amount,
    peer: Amount,
    history: Amount,
    active_jobs: usize,
    remote_active_jobs: usize,
    peer_active_jobs: usize,
    pub(super) per_job: MaterializationLimits,
    pub(super) workers: usize,
    pub(super) max_block_bytes: usize,
    pub(super) max_owners: usize,
}

impl Limits {
    pub(super) fn new(config: &TxPoolConfig, consensus: &Consensus) -> Result<Self, Error> {
        let residency = ResidencyLimits::from_pool_size(config.max_tx_pool_size)
            .ok_or_else(|| Error::Full("transaction-pool residency bound overflow".into()))?;
        Self::with_residency(config, consensus, residency)
    }
    pub(super) fn with_residency(
        config: &TxPoolConfig,
        consensus: &Consensus,
        residency: ResidencyLimits,
    ) -> Result<Self, Error> {
        let bad = || Error::Full("invalid transaction-pool resource configuration".into());
        let workers = config.max_tx_verify_workers.max(1);
        let remote_items = workers.checked_add(1).ok_or_else(bad)?;
        let active_items = remote_items.checked_add(1).ok_or_else(bad)?;
        if active_items > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(bad());
        }
        let pipeline_bytes = residency.pipeline;
        let max_block_bytes = usize::try_from(consensus.max_block_bytes()).map_err(|_| bad())?;
        let raw_edges = pipeline_bytes / 160;
        // Keep one job for each VM worker, one resolver and trusted headroom.
        // The last envelope comes from queued residency, not a smaller job cap.
        let job_bytes = (pipeline_bytes / 4)
            .checked_div(remote_items)
            .ok_or_else(bad)?;
        let job_edges = (raw_edges / 4)
            .checked_div(remote_items)
            .ok_or_else(bad)?
            .min(
                max_block_bytes
                    .checked_div(32)
                    .and_then(|n| n.checked_add(1))
                    .ok_or_else(bad)?,
            );
        if job_bytes <= ENTRY_BYTES
            || job_edges == 0
            || config.max_ancestors_count == 0
            || config.tx_verify_cycles_per_ms == 0
            || config.min_tx_verify_time_ms == 0
            || config.min_tx_verify_time_ms > config.max_tx_verify_time_ms
            || config.max_tx_verify_initial_load_bytes == 0
        {
            return Err(bad());
        }
        let active = MaterializationLimits {
            bytes: job_bytes.checked_mul(active_items).ok_or_else(bad)?,
            edges: job_edges.checked_mul(active_items).ok_or_else(bad)?,
        };
        let pipeline = Amount {
            items: pipeline_bytes / 768,
            bytes: pipeline_bytes.checked_sub(active.bytes).ok_or_else(bad)?,
            edges: raw_edges.checked_sub(active.edges).ok_or_else(bad)?,
            ..Amount::default()
        };
        let remote = pipeline.fraction(7, 8).ok_or_else(bad)?;
        // The validated job envelope already leaves nonzero queued peer shares.
        let peer = remote.fraction(1, 8).ok_or_else(bad)?;
        let peer_items = remote_items.div_ceil(4);
        let history = Amount {
            items: (pipeline.items / 16).clamp(1, 10_000),
            bytes: (pipeline.bytes / 16).clamp(1, 50_000_000),
            edges: (pipeline.edges / 16).max(1),
            ..Amount::default()
        };
        let accepted = Amount {
            items: residency.accepted / 1_024,
            bytes: residency.accepted,
            edges: usize::MAX,
            serialized: config.max_tx_pool_size,
            cycles: u64::MAX,
        };
        let max_owners = accepted.items.checked_add(pipeline.items).ok_or_else(bad)?;
        Ok(Self {
            accepted,
            pipeline,
            remote,
            peer,
            history,
            active_jobs: active_items,
            remote_active_jobs: remote_items,
            peer_active_jobs: peer_items,
            per_job: MaterializationLimits {
                bytes: job_bytes,
                edges: job_edges,
            },
            workers,
            max_block_bytes,
            max_owners,
        })
    }
    fn for_account(&self, account: Account) -> Amount {
        match account {
            Account::Accepted => self.accepted,
            Account::Pipeline => self.pipeline,
            Account::Remote => self.remote,
            Account::Peer(_) => self.peer,
            Account::History => self.history,
        }
    }
    pub(super) fn resolved_fits(&self, entry: &Entry) -> Result<(), Error> {
        let amount = owner_amount(entry)?;
        if amount.bytes > self.per_job.bytes || amount.edges > self.per_job.edges {
            return Err(Error::Full(
                "transaction exceeds the active materialization envelope".into(),
            ));
        }
        Ok(())
    }
    /// Select a bounded recovery population in the caller's dependency order.
    /// This is pure planning; Store still reserves and commits the exact delta.
    pub(super) fn retain_fitting(&self, entries: &mut Vec<Arc<Entry>>) -> Result<(), Error> {
        let mut totals = BTreeMap::<Account, Amount>::new();
        let mut failure = None;
        entries.retain(|entry| {
            let mut charges = match owner_accounts(entry) {
                Ok(charges) => charges,
                Err(error) => {
                    failure = Some(error);
                    return false;
                }
            };
            for (account, charge) in charges.iter_mut().flatten() {
                let Some(projected) = totals
                    .get(account)
                    .copied()
                    .unwrap_or_default()
                    .checked_add(*charge)
                else {
                    failure = Some(Error::Full("recovery accounting arithmetic".into()));
                    return false;
                };
                if !projected.fits(self.for_account(*account)) {
                    return false;
                }
                *charge = projected;
            }
            totals.extend(charges.into_iter().flatten());
            true
        });
        failure.map_or(Ok(()), Err)
    }
}

pub(super) fn owner_amount(entry: &Entry) -> Result<Amount, Error> {
    let tx = &entry.transaction;
    let raw_edges = tx
        .inputs()
        .len()
        .checked_add(tx.cell_deps().len())
        .and_then(|n| n.checked_add(tx.header_deps().len()))
        .ok_or(Error::Full("transaction edge arithmetic".into()))?;
    let mut edges = raw_edges;
    let mut bytes = tx
        .data()
        .total_size()
        .checked_add(ENTRY_BYTES)
        .ok_or(Error::Full("transaction byte arithmetic".into()))?;
    let mut serialized = 0;
    let mut cycles = 0;
    match &entry.phase {
        Phase::Accepted(accepted) => {
            edges = entry
                .dependencies()
                .len()
                .checked_add(accepted.parents.len())
                .ok_or(Error::Full("accepted edge arithmetic".into()))?;
            bytes =
                accepted_transaction_charge_bytes(tx.data().total_size(), &accepted.transaction)
                    .checked_add(
                        accepted
                            .parents
                            .len()
                            .checked_mul(160)
                            .ok_or(Error::Full("parent byte arithmetic".into()))?,
                    )
                    .ok_or(Error::Full("accepted byte arithmetic".into()))?;
            serialized = accepted.size;
            cycles = accepted.cycles;
        }
        Phase::Verify(resolved) => {
            edges = entry.dependencies().len().max(raw_edges);
            bytes =
                resolved_transaction_charge_bytes(tx.data().total_size(), &resolved.transaction)
                    .checked_add(ENTRY_BYTES)
                    .ok_or(Error::Full("resolved byte arithmetic".into()))?;
        }
        Phase::Waiting(keys) | Phase::Replaced { triggers: keys, .. } => {
            edges = edges.max(keys.len());
        }
        Phase::Resolve => {}
    }
    if !matches!(entry.phase, Phase::Accepted(_)) {
        bytes = bytes
            .checked_add(
                edges
                    .checked_mul(EDGE_BYTES)
                    .ok_or(Error::Full("edge byte arithmetic".into()))?,
            )
            .ok_or(Error::Full("owner byte arithmetic".into()))?;
    }
    Ok(Amount {
        items: 1,
        bytes,
        edges,
        serialized,
        cycles,
    })
}

// One owner charges Accepted, Pipeline+History, or Pipeline plus its remote
// and peer accounts. Keep this bounded routing on the stack during planning.
fn owner_accounts(entry: &Entry) -> Result<[Option<(Account, Amount)>; 3], Error> {
    let amount = owner_amount(entry)?;
    let charge = |account| Some((account, amount));
    if entry.accepted().is_some() {
        return Ok([charge(Account::Accepted), None, None]);
    }
    let (shared, peer) = if matches!(entry.phase, Phase::Replaced { .. }) {
        (charge(Account::History), None)
    } else if let Some(peer) = entry.source.residency_peer() {
        (charge(Account::Remote), charge(Account::Peer(peer)))
    } else {
        (None, None)
    };
    Ok([charge(Account::Pipeline), shared, peer])
}

/// Exact owner charges prepared without holding authority guards or capacity.
/// Aggregate both sides together, then retain only each account's net changes.
pub(super) struct OwnerDelta {
    positive: Vec<(Account, Amount)>,
    negative: Vec<(Account, Amount)>,
}

impl OwnerDelta {
    pub(super) fn new<'a>(
        before: impl Iterator<Item = &'a Entry>,
        after: impl Iterator<Item = &'a Entry>,
    ) -> Result<Self, Error> {
        let mut totals = BTreeMap::<Account, (Amount, Amount)>::new();
        for entry in before {
            for (account, amount) in owner_accounts(entry)?.into_iter().flatten() {
                let (old, _) = totals.entry(account).or_default();
                *old = old
                    .checked_add(amount)
                    .ok_or(Error::Full("quota arithmetic".into()))?;
            }
        }
        for entry in after {
            for (account, amount) in owner_accounts(entry)?.into_iter().flatten() {
                let (_, new) = totals.entry(account).or_default();
                *new = new
                    .checked_add(amount)
                    .ok_or(Error::Full("quota arithmetic".into()))?;
            }
        }
        let mut positive = Vec::new();
        let mut negative = Vec::new();
        for (account, (old, new)) in totals {
            let added = new.positive_difference(old);
            let removed = old.positive_difference(new);
            if added != Amount::default() {
                positive.push((account, added));
            }
            if removed != Amount::default() {
                negative.push((account, removed));
            }
        }
        Ok(Self { positive, negative })
    }

    /// The caller holds the validated owner cut until this reservation commits
    /// or drops, so a complete owner capture cannot observe unsettled capacity.
    pub(super) fn reserve(self, budget: &Arc<Budget>) -> Result<Reservation, Error> {
        budget.reserve_changes(self.positive, self.negative)
    }
}

// Every active permit reserves the same immutable per_job envelope. Counts
// therefore bound items, bytes and edges together; owner quotas are disjoint.
#[derive(Default)]
struct Active {
    total: usize,
    remote: usize,
    peers: BTreeMap<PeerIndex, usize>,
}
impl Active {
    fn reserve(&mut self, peer: Option<PeerIndex>, limits: &Limits) -> Result<(), Error> {
        let total = self
            .total
            .checked_add(1)
            .filter(|total| *total <= limits.active_jobs)
            .ok_or(Error::Full(FullReason::Active))?;
        if let Some(peer) = peer {
            let remote = self
                .remote
                .checked_add(1)
                .filter(|remote| *remote <= limits.remote_active_jobs)
                .ok_or(Error::Full(FullReason::Other("remote active work")))?;
            let count = self
                .peers
                .get(&peer)
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .filter(|count| *count <= limits.peer_active_jobs)
                .ok_or(Error::Full(FullReason::Other("peer active work")))?;
            // All expected refusals precede the first counter change.
            self.peers.insert(peer, count);
            self.remote = remote;
        }
        self.total = total;
        Ok(())
    }
    fn release(&mut self, peer: Option<PeerIndex>) -> Option<()> {
        let total = self.total.checked_sub(1)?;
        if let Some(peer) = peer {
            let remote = self.remote.checked_sub(1)?;
            let count = self.peers.get(&peer)?.checked_sub(1)?;
            if count == 0 {
                self.peers.remove(&peer);
            } else {
                self.peers.insert(peer, count);
            }
            self.remote = remote;
        }
        self.total = total;
        Some(())
    }
}

/// One non-Clone activity envelope, held through computation and settlement.
/// Its original peer identity stays fixed even if the owner is superseded.
pub(super) struct ActivePermit {
    budget: Arc<Budget>,
    peer: Option<PeerIndex>,
}
impl Drop for ActivePermit {
    fn drop(&mut self) {
        if self.budget.active.lock().release(self.peer).is_none() {
            self.budget.faulted.store(true, Ordering::Release);
        }
        self.budget.changed.notify_waiters();
    }
}

pub(super) struct Budget {
    pub(super) limits: Limits,
    usage: Mutex<BTreeMap<Account, Amount>>,
    active: Mutex<Active>,
    faulted: AtomicBool,
    pub(super) changed: Notify,
}

pub(super) struct Reservation {
    budget: Arc<Budget>,
    positive: Option<Vec<(Account, Amount)>>,
    negative: Vec<(Account, Amount)>,
}

impl Budget {
    pub(super) fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            usage: Mutex::new(BTreeMap::new()),
            active: Mutex::new(Active::default()),
            faulted: AtomicBool::new(false),
            changed: Notify::new(),
        })
    }
    pub(super) fn faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire)
    }
    pub(super) fn publish_metrics(&self) {
        let mut snapshot = {
            let usage = self.usage.lock();
            let get = |account| usage.get(&account).copied().unwrap_or_default();
            let pipeline = get(Account::Pipeline);
            let remote = get(Account::Remote);
            let history = get(Account::History);
            crate::metrics::KernelUsage {
                total_entries: pipeline.items,
                total_bytes: pipeline.bytes,
                remote_entries: remote.items,
                remote_bytes: remote.bytes,
                conflict_entries: history.items,
                conflict_bytes: history.bytes,
                active_work: 0,
            }
        };
        snapshot.active_work = self.active.lock().total;
        snapshot.publish();
    }
    pub(super) fn accepted_usage(&self) -> Amount {
        self.usage
            .lock()
            .get(&Account::Accepted)
            .copied()
            .unwrap_or_default()
    }
    pub(super) fn active(self: &Arc<Self>, source: Source) -> Result<ActivePermit, Error> {
        if self.faulted() {
            return Err(Error::Fault("quota counter"));
        }
        let peer = source.compute_peer();
        self.active.lock().reserve(peer, &self.limits)?;
        Ok(ActivePermit {
            budget: Arc::clone(self),
            peer,
        })
    }
    fn reserve_changes(
        self: &Arc<Self>,
        positive: Vec<(Account, Amount)>,
        negative: Vec<(Account, Amount)>,
    ) -> Result<Reservation, Error> {
        if self.faulted() {
            return Err(Error::Fault("quota counter"));
        }
        let mut usage = self.usage.lock();
        for (key, delta) in &positive {
            let projected = usage
                .get(key)
                .copied()
                .unwrap_or_default()
                .checked_add(*delta);
            if projected.is_none_or(|amount| !amount.fits(self.limits.for_account(*key))) {
                return Err(Error::Full(match key {
                    Account::Accepted => FullReason::Accepted,
                    Account::Pipeline => FullReason::Pipeline,
                    Account::Remote => FullReason::Other("remote pipeline"),
                    Account::Peer(_) => FullReason::Other("peer pipeline"),
                    Account::History => FullReason::History,
                }));
            }
        }
        for (key, delta) in &positive {
            let before = usage.get(key).copied().unwrap_or_default();
            // Same guard and operands as preflight; an impossible failure faults
            // the generation rather than exposing a policy rejection prefix.
            if let Some(after) = before.checked_add(*delta) {
                usage.insert(*key, after);
            } else {
                self.faulted.store(true, Ordering::Release);
            }
        }
        drop(usage);
        Ok(Reservation {
            budget: Arc::clone(self),
            positive: Some(positive),
            negative,
        })
    }
    fn release(&self, amounts: &[(Account, Amount)], notify: bool) {
        if amounts.is_empty() {
            return;
        }
        let mut usage = self.usage.lock();
        for (key, amount) in amounts {
            let old = usage.get(key).copied().unwrap_or_default();
            if let Some(new) = old.checked_sub(*amount) {
                if new == Amount::default() {
                    usage.remove(key);
                } else {
                    usage.insert(*key, new);
                }
            } else {
                self.faulted.store(true, Ordering::Release);
            }
        }
        drop(usage);
        if notify {
            self.changed.notify_waiters();
        }
    }
}

impl Reservation {
    /// The owner cut has validated exact victims. Convert positive reservation
    /// to live charge and release only the negative side of the same edit.
    /// Report returned capacity so Store wakes waiters after releasing guards.
    pub(super) fn commit(mut self) -> bool {
        self.positive.take();
        self.budget.release(&self.negative, false);
        !self.negative.is_empty()
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(positive) = self.positive.take() {
            self.budget.release(&positive, true);
        }
    }
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct OwnerUsage {
    pub(super) accepted: Amount,
    pub(super) pipeline: Amount,
    pub(super) remote: Amount,
    pub(super) history: Amount,
    pub(super) peers: BTreeMap<PeerIndex, Amount>,
}

#[cfg(test)]
impl Budget {
    /// Observe settled accounting without reusing owner routing or delta preparation.
    pub(super) fn owner_usage(&self) -> OwnerUsage {
        let usage = self.usage.lock();
        let amount = |account| usage.get(&account).copied().unwrap_or_default();
        OwnerUsage {
            accepted: amount(Account::Accepted),
            pipeline: amount(Account::Pipeline),
            remote: amount(Account::Remote),
            history: amount(Account::History),
            peers: usage
                .iter()
                .filter_map(|(account, amount)| match account {
                    Account::Peer(peer) => Some((*peer, *amount)),
                    _ => None,
                })
                .collect(),
        }
    }
}
