//! Checked owner charges, inline authority-state accounting and active-job limits.
//!
//! Owner usage lives directly inside `AuthorityState` so every quota check and
//! live update settles atomically under the same write guard with zero two-phase
//! reservation locking. Active off-lock work retains its fixed envelope through
//! `ActivePermit`.
use super::model::{Entry, Error, FullReason, OutPointKey, Phase, Source};
use super::residency::{accepted_transaction_charge_bytes, resolved_transaction_charge_bytes};
use super::service::CapacitySignal;
use crate::constants::ResidencyLimits;
use ckb_app_config::TxPoolConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_network::PeerIndex;
use ckb_types::{core::TransactionView, packed::Byte32};
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, OnceLock,
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

/// A bounded observation made while preparing a rejection.
#[derive(Clone, Copy)]
pub(super) struct AccountSnapshot {
    pub(super) account: &'static str,
    pub(super) usage: Amount,
    pub(super) limit: Amount,
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

    pub(super) fn positive_difference(self, rhs: Self) -> Self {
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

    fn retention_share(self, numerator: usize, denominator: usize) -> Option<Self> {
        // Subquotas partition retained resources; serialized bytes and cycles
        // keep the whole-pool ceilings rather than becoming peer admission policy.
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
pub(super) enum Account {
    Accepted,
    Pipeline,
    Remote,
    Peer(PeerIndex),
    History,
}

impl Account {
    fn full_reason(self) -> FullReason {
        match self {
            Self::Accepted => FullReason::Accepted,
            Self::Pipeline => FullReason::Pipeline,
            Self::Remote => FullReason::Other("remote pipeline"),
            Self::Peer(_) => FullReason::Other("peer pipeline"),
            Self::History => FullReason::History,
        }
    }
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
    pub(super) remote: Amount,
    pub(super) peer: Amount,
    pub(super) history: Amount,
    pub(super) active_jobs: usize,
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
        let bad = || Error::Full("transaction-pool resource bound arithmetic overflow".into());
        let workers = config.max_tx_verify_workers;
        let active_items = workers
            .checked_add(2)
            .filter(|slots| *slots <= tokio::sync::Semaphore::MAX_PERMITS)
            .ok_or_else(|| {
                let reason = "max_tx_verify_workers exceeds the supported worker count";
                ckb_logger::error!(
                    "{reason}: {workers}; maximum is {}",
                    tokio::sync::Semaphore::MAX_PERMITS.saturating_sub(2),
                );
                Error::Full(reason.into())
            })?;
        let remote_items = active_items.checked_sub(1).ok_or_else(bad)?;
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
        if job_bytes <= ENTRY_BYTES || job_edges == 0 {
            return Err(Error::Full("per-job memory is too small; reduce max_tx_verify_workers or increase max_tx_pool_size".into()));
        }
        if config.max_ancestors_count == 0 {
            return Err(Error::Full("max_ancestors_count must be positive".into()));
        }
        if config.max_tx_verify_time().is_zero() {
            return Err(Error::Full(
                "internal network verification time cap must be positive (test override)".into(),
            ));
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
        let remote = pipeline.retention_share(7, 8).ok_or_else(bad)?;
        let peer = remote.retention_share(1, 8).ok_or_else(bad)?;
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

    pub(super) fn for_account(&self, account: Account) -> Amount {
        match account {
            Account::Accepted => self.accepted,
            Account::Pipeline => self.pipeline,
            Account::Remote => self.remote,
            Account::Peer(_) => self.peer,
            Account::History => self.history,
        }
    }

    pub(super) fn resolved_fits(&self, entry: &Entry) -> Result<(), Error> {
        self.resolved_amount_fits(entry.charge())
    }

    pub(super) fn resolved_amount_fits(&self, amount: Amount) -> Result<(), Error> {
        if amount.bytes > self.per_job.bytes || amount.edges > self.per_job.edges {
            return Err(Error::Full(
                "transaction exceeds the active materialization envelope".into(),
            ));
        }
        Ok(())
    }

    /// Select a bounded recovery population in the caller's dependency order.
    pub(super) fn retain_fitting(&self, entries: &mut Vec<Arc<Entry>>) -> Result<(), Error> {
        let mut usage = AccountUsage::default();
        let mut failure = None;
        entries.retain(|entry| {
            let accounts = owner_accounts(entry);
            for (account, charge) in accounts.iter().flatten() {
                let Some(projected) = usage.get(*account).checked_add(*charge) else {
                    failure = Some(Error::Full("recovery accounting arithmetic".into()));
                    return false;
                };
                if !projected.fits(self.for_account(*account)) {
                    return false;
                }
            }
            for (account, charge) in accounts.into_iter().flatten() {
                if usage.add_account(account, charge).is_err() {
                    failure = Some(Error::Full("recovery accounting arithmetic".into()));
                    return false;
                }
            }
            true
        });
        failure.map_or(Ok(()), Err)
    }
}

/// Count unique items without heap allocation when the set fits in `STACK` slots.
fn count_unique<const STACK: usize, T: Eq + std::hash::Hash>(
    iter: impl Iterator<Item = T>,
) -> usize {
    let mut small: [Option<T>; STACK] = std::array::from_fn(|_| None);
    let mut len = 0usize;
    let mut spilled: Option<HashSet<T>> = None;
    for item in iter {
        if let Some(set) = &mut spilled {
            set.insert(item);
            continue;
        }
        if small.get(..len).is_some_and(|slice| {
            slice
                .iter()
                .any(|existing| existing.as_ref() == Some(&item))
        }) {
            continue;
        }
        if let Some(slot) = small.get_mut(len) {
            *slot = Some(item);
            len = len.saturating_add(1);
        } else {
            let mut set = HashSet::with_capacity(STACK.saturating_mul(2));
            for existing in small.iter_mut().filter_map(Option::take) {
                set.insert(existing);
            }
            set.insert(item);
            spilled = Some(set);
        }
    }
    spilled.map_or(len, |set| set.len())
}

fn raw_transaction_edges(tx: &TransactionView) -> Result<usize, Error> {
    tx.inputs()
        .len()
        .checked_add(tx.cell_deps().len())
        .and_then(|n| n.checked_add(tx.header_deps().len()))
        .ok_or(Error::Full("transaction edge arithmetic".into()))
}

fn finish_preaccepted_amount(base_bytes: usize, edges: usize) -> Result<Amount, Error> {
    let bytes = base_bytes
        .checked_add(
            edges
                .checked_mul(EDGE_BYTES)
                .ok_or(Error::Full("edge byte arithmetic".into()))?,
        )
        .ok_or(Error::Full("owner byte arithmetic".into()))?;
    Ok(Amount {
        items: 1,
        bytes,
        edges,
        serialized: 0,
        cycles: 0,
    })
}

/// Charge for a resolved `Phase::Verify` owner without allocating a temporary `Entry`.
pub(super) fn compute_resolved_amount(
    tx: &TransactionView,
    rtx: &ckb_types::core::cell::ResolvedTransaction,
) -> Result<Amount, Error> {
    let raw_edges = raw_transaction_edges(tx)?;
    let unique_cells = count_unique::<16, OutPointKey>(
        rtx.resolved_inputs
            .iter()
            .chain(&rtx.resolved_cell_deps)
            .chain(&rtx.resolved_dep_groups)
            .map(|cell| OutPointKey::from_out_point(&cell.out_point)),
    );
    let unique_headers = count_unique::<8, Byte32>(tx.header_deps_iter());
    let edges = unique_cells
        .checked_add(unique_headers)
        .ok_or(Error::Full("resolved edge arithmetic".into()))?
        .max(raw_edges);
    let base_bytes = resolved_transaction_charge_bytes(tx.data().total_size(), rtx)
        .checked_add(ENTRY_BYTES)
        .ok_or(Error::Full("resolved byte arithmetic".into()))?;
    finish_preaccepted_amount(base_bytes, edges)
}

/// Charge for a `Phase::Waiting` / `Phase::Replaced` owner without allocating a temporary `Entry`.
pub(super) fn compute_waiting_amount(
    tx: &TransactionView,
    wait_keys: usize,
) -> Result<Amount, Error> {
    let edges = raw_transaction_edges(tx)?.max(wait_keys);
    let base_bytes = tx
        .data()
        .total_size()
        .checked_add(ENTRY_BYTES)
        .ok_or(Error::Full("transaction byte arithmetic".into()))?;
    finish_preaccepted_amount(base_bytes, edges)
}

/// Base resident charge for an accepted owner with 0 parents, computed off-lock
/// so `commit_admission` only adds `parents.len()` inside `state.write()`.
pub(super) fn compute_accepted_base_amount(
    tx: &TransactionView,
    accepted_rtx: &ckb_types::core::cell::ResolvedTransaction,
    size: usize,
    cycles: u64,
) -> Result<Amount, Error> {
    let unique_cells = count_unique::<16, OutPointKey>(
        accepted_rtx
            .resolved_inputs
            .iter()
            .map(|cell| OutPointKey::from_out_point(&cell.out_point))
            .chain(
                accepted_rtx
                    .related_dep_out_points()
                    .map(OutPointKey::from_out_point),
            ),
    );
    let unique_headers = count_unique::<8, Byte32>(tx.header_deps_iter());
    let edges = unique_cells
        .checked_add(unique_headers)
        .ok_or(Error::Full("accepted edge arithmetic".into()))?;
    let bytes = accepted_transaction_charge_bytes(tx.data().total_size(), accepted_rtx);
    Ok(Amount {
        items: 1,
        bytes,
        edges,
        serialized: size,
        cycles,
    })
}

/// Complete an accepted owner's resident charge from its precomputed base
/// charge and final parent count in O(1).
pub(super) fn finish_accepted_amount(base: Amount, parent_count: usize) -> Result<Amount, Error> {
    let edges = base
        .edges
        .checked_add(parent_count)
        .ok_or(Error::Full("accepted edge arithmetic".into()))?;
    let bytes = base
        .bytes
        .checked_add(
            parent_count
                .checked_mul(160)
                .ok_or(Error::Full("parent byte arithmetic".into()))?,
        )
        .ok_or(Error::Full("accepted byte arithmetic".into()))?;
    Ok(Amount {
        edges,
        bytes,
        ..base
    })
}

pub(super) fn compute_owner_amount(tx: &TransactionView, phase: &Phase) -> Result<Amount, Error> {
    match phase {
        Phase::Accepted(accepted) => {
            let base = compute_accepted_base_amount(
                tx,
                &accepted.transaction,
                accepted.size,
                accepted.scripts.cycles(),
            )?;
            finish_accepted_amount(base, accepted.parents.len())
        }
        Phase::Verify(resolved) => compute_resolved_amount(tx, &resolved.transaction),
        Phase::Waiting(keys) => compute_waiting_amount(tx, keys.len()),
        Phase::Replaced(triggers) => compute_waiting_amount(tx, triggers.keys().len()),
        Phase::Resolve => compute_waiting_amount(tx, 0),
    }
}

#[cfg(test)]
pub(super) fn owner_amount(entry: &Entry) -> Amount {
    entry.charge()
}

/// One owner charges Accepted, Pipeline+History, or Pipeline plus its remote
/// and peer accounts. Bounded stack routing with zero allocation.
pub(super) fn owner_accounts(entry: &Entry) -> [Option<(Account, Amount)>; 3] {
    let amount = entry.charge();
    let charge = |account| Some((account, amount));
    if let Some(peer) = entry.preaccepted_peer() {
        return [
            charge(Account::Pipeline),
            charge(Account::Remote),
            charge(Account::Peer(peer)),
        ];
    }
    match &entry.phase {
        Phase::Accepted(_) => [charge(Account::Accepted), None, None],
        Phase::Replaced(_) => [charge(Account::Pipeline), charge(Account::History), None],
        _ => [charge(Account::Pipeline), None, None],
    }
}

/// Inline owner accounting owned directly by `AuthorityState`.
///
/// The four fixed accounts sit inline with zero map lookups; peer subquotas
/// live in a compact map cleaned up whenever a peer reaches zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AccountUsage {
    pub(super) accepted: Amount,
    pub(super) pipeline: Amount,
    pub(super) remote: Amount,
    pub(super) history: Amount,
    pub(super) peers: HashMap<PeerIndex, Amount>,
}

impl AccountUsage {
    pub(super) fn get(&self, account: Account) -> Amount {
        match account {
            Account::Accepted => self.accepted,
            Account::Pipeline => self.pipeline,
            Account::Remote => self.remote,
            Account::History => self.history,
            Account::Peer(peer) => self.peers.get(&peer).copied().unwrap_or_default(),
        }
    }

    fn add_account(&mut self, account: Account, delta: Amount) -> Result<(), Error> {
        let next = self
            .get(account)
            .checked_add(delta)
            .ok_or(Error::Fault("quota counter"))?;
        self.set(account, next);
        Ok(())
    }

    fn sub_account(&mut self, account: Account, delta: Amount) -> Result<(), Error> {
        let next = self
            .get(account)
            .checked_sub(delta)
            .ok_or(Error::Fault("quota counter"))?;
        self.set(account, next);
        Ok(())
    }

    fn set(&mut self, account: Account, value: Amount) {
        match account {
            Account::Accepted => self.accepted = value,
            Account::Pipeline => self.pipeline = value,
            Account::Remote => self.remote = value,
            Account::History => self.history = value,
            Account::Peer(peer) => {
                if value == Amount::default() {
                    self.peers.remove(&peer);
                } else {
                    self.peers.insert(peer, value);
                }
            }
        }
    }

    fn check_account_delta(
        &self,
        account: Account,
        removed: Amount,
        added: Amount,
        limits: &Limits,
    ) -> Result<(), Error> {
        let projected = self
            .get(account)
            .checked_sub(removed)
            .ok_or(Error::Fault("quota counter"))?
            .checked_add(added);
        if projected.is_none_or(|amount| !amount.fits(limits.for_account(account))) {
            return Err(Error::Full(account.full_reason()));
        }
        Ok(())
    }

    /// Check whether replacing `before` with `after` fits all account limits
    /// without mutating live usage.
    pub(super) fn check_delta<'a>(
        &self,
        before: impl Iterator<Item = &'a Entry>,
        after: impl Iterator<Item = &'a Entry>,
        limits: &Limits,
    ) -> Result<(), Error> {
        let mut totals = BTreeMap::<Account, (Amount, Amount)>::new();
        let mut accumulate = |entry: &Entry, is_after: bool| -> Result<(), Error> {
            for (account, amount) in owner_accounts(entry).into_iter().flatten() {
                let pair = totals.entry(account).or_default();
                let slot = if is_after { &mut pair.1 } else { &mut pair.0 };
                *slot = slot
                    .checked_add(amount)
                    .ok_or(Error::Full("quota arithmetic".into()))?;
            }
            Ok(())
        };
        for entry in before {
            accumulate(entry, false)?;
        }
        for entry in after {
            accumulate(entry, true)?;
        }
        for (account, (old, new)) in totals {
            self.check_account_delta(
                account,
                old.positive_difference(new),
                new.positive_difference(old),
                limits,
            )?;
        }
        Ok(())
    }

    /// Fast O(1) check for a single owner edit without allocating a map.
    pub(super) fn check_single(
        &self,
        before: Option<&Entry>,
        after: Option<&Entry>,
        limits: &Limits,
    ) -> Result<(), Error> {
        let old_accounts = before.map(owner_accounts).unwrap_or_default();
        let new_accounts = after.map(owner_accounts).unwrap_or_default();
        for (account, new_amount) in new_accounts.into_iter().flatten() {
            let old_amount = old_accounts
                .iter()
                .flatten()
                .find(|(a, _)| *a == account)
                .map_or(Amount::default(), |(_, amount)| *amount);
            let added = new_amount.positive_difference(old_amount);
            if added != Amount::default() {
                self.check_account_delta(
                    account,
                    old_amount.positive_difference(new_amount),
                    added,
                    limits,
                )?;
            }
        }
        Ok(())
    }

    /// Check whether additional replacement-history entries fit both `History`
    /// and `Pipeline` given `released_pipeline` already being freed in the same
    /// admission (e.g., candidate moving from Verify/Resolve -> Accepted).
    pub(super) fn history_fits(
        &self,
        history_charge: Amount,
        released_pipeline: Amount,
        limits: &Limits,
    ) -> bool {
        let Some(projected_history) = self.history.checked_add(history_charge) else {
            return false;
        };
        if !projected_history.fits(limits.history) {
            return false;
        }
        let Some(projected_pipeline) = self
            .pipeline
            .checked_sub(released_pipeline)
            .and_then(|base| base.checked_add(history_charge))
        else {
            return false;
        };
        projected_pipeline.fits(limits.pipeline)
    }

    /// Apply one validated owner edit to live usage.
    pub(super) fn apply_edit(
        &mut self,
        before: Option<&Entry>,
        after: Option<&Entry>,
    ) -> Result<(), Error> {
        if let Some(before) = before {
            for (account, amount) in owner_accounts(before).into_iter().flatten() {
                self.sub_account(account, amount)?;
            }
        }
        if let Some(after) = after {
            for (account, amount) in owner_accounts(after).into_iter().flatten() {
                self.add_account(account, amount)?;
            }
        }
        Ok(())
    }

    /// Snapshot the four fixed accounts plus any peer accounts touched by an
    /// upcoming transition so we can report accurately whether net capacity was
    /// returned without waking maintenance on equal-sized replacements.
    pub(super) fn has_returned_capacity_since(
        &self,
        before_fixed: [Amount; 4],
        mut before_peers: impl Iterator<Item = (PeerIndex, Amount)>,
    ) -> bool {
        let after_fixed = [self.accepted, self.pipeline, self.remote, self.history];
        if before_fixed
            .into_iter()
            .zip(after_fixed)
            .any(|(old, new)| old.positive_difference(new) != Amount::default())
        {
            return true;
        }
        before_peers.any(|(peer, old)| {
            let new = self.peers.get(&peer).copied().unwrap_or_default();
            old.positive_difference(new) != Amount::default()
        })
    }

    pub(super) fn fixed_snapshot(&self) -> [Amount; 4] {
        [self.accepted, self.pipeline, self.remote, self.history]
    }

    pub(super) fn rejection_snapshot(
        &self,
        limits: &Limits,
        peer: Option<PeerIndex>,
    ) -> [Option<AccountSnapshot>; 5] {
        [
            Some(("accepted", Account::Accepted)),
            Some(("pipeline", Account::Pipeline)),
            Some(("remote", Account::Remote)),
            peer.map(|peer| ("peer", Account::Peer(peer))),
            Some(("history", Account::History)),
        ]
        .map(|account| {
            account.map(|(account, key)| AccountSnapshot {
                account,
                usage: self.get(key),
                limit: limits.for_account(key),
            })
        })
    }

    #[cfg(test)]
    pub(super) fn to_owner_usage(&self) -> OwnerUsage {
        OwnerUsage {
            accepted: self.accepted,
            pipeline: self.pipeline,
            remote: self.remote,
            history: self.history,
            peers: self.peers.iter().map(|(k, v)| (*k, *v)).collect(),
        }
    }
}

// Every active permit reserves the same immutable per_job envelope. Counts
// therefore bound items, bytes and edges together; owner quotas live in AuthorityState.
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
    budget: Arc<ActiveBudget>,
    peer: Option<PeerIndex>,
}

impl Drop for ActivePermit {
    fn drop(&mut self) {
        if self.budget.active.lock().release(self.peer).is_none() {
            self.budget.faulted.store(true, Ordering::SeqCst);
        }
        self.budget.active_changed.notify_waiters();
        if let Some(signal) = self.budget.capacity_signal.get() {
            signal.notify();
        }
    }
}

pub(super) struct ActiveBudget {
    pub(super) limits: Limits,
    active: Mutex<Active>,
    faulted: Arc<AtomicBool>,
    pub(super) active_changed: Notify,
    capacity_signal: OnceLock<CapacitySignal>,
}

impl ActiveBudget {
    pub(super) fn new(limits: Limits, faulted: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            limits,
            active: Mutex::new(Active::default()),
            faulted,
            active_changed: Notify::new(),
            capacity_signal: OnceLock::new(),
        })
    }

    pub(super) fn attach_capacity_signal(&self, signal: CapacitySignal) {
        let _ = self.capacity_signal.set(signal);
    }

    pub(super) fn faulted(&self) -> bool {
        self.faulted.load(Ordering::SeqCst)
    }

    pub(super) fn active_total(&self) -> usize {
        self.active.lock().total
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

    #[cfg(test)]
    pub(super) fn active_is_empty_for_test(&self) -> bool {
        let active = self.active.lock();
        active.total == 0 && active.remote == 0 && active.peers.is_empty()
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
