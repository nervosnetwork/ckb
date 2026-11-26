//! Public values derived from coherent immutable owners. Full projections are
//! called only while a service read slot owns their bounded scratch lifetime.
use super::{
    budget::Amount,
    membership::{self, Aggregate, NeighborhoodTotals},
    model::{Entry, Error, OutPointKey, Phase, Status, hash_key},
    packing::Selection,
    ranks::{Priority, RankedPriorities},
    store::{Captured, Store},
};
use crate::{component::sort_key::AncestorsScoreSortKey, util::compact_packed};
use ckb_app_config::TxPoolConfig;
use ckb_fee_estimator::FeeSample;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    bytes::Bytes,
    core::{
        Capacity, Cycle, FeeRate, TransactionView,
        cell::{CellMeta, CellProvider, CellStatus},
        tx_pool::{
            PoolTxDetailInfo, TRANSACTION_SIZE_LIMIT, TransactionWithStatus, TxEntryInfo,
            TxPoolEntryInfo, TxPoolIds, TxPoolInfo, TxStatus, get_transaction_weight,
        },
    },
    packed::{Byte32, CellOutput, OutPoint, ProposalShortId},
    prelude::*,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Cache only the compact public projection. The accepted population revision and
/// the paired lifecycle view decide reuse; queued work cannot invalidate it.
#[derive(Default)]
pub(super) struct InputSnapshotCache(ckb_util::Mutex<Option<CachedInputs>>);

struct CachedInputs {
    view: u64,
    accepted_revision: u64,
    value: crate::TxPoolInputSnapshot,
}

impl InputSnapshotCache {
    pub(super) fn capture(&self, store: &Store) -> Result<crate::TxPoolInputSnapshot, Error> {
        let mut cached = self.0.lock();
        if let Some(previous) = cached.as_ref() {
            match store.check_accepted_revision(previous.view, previous.accepted_revision) {
                Ok(()) => return Ok(previous.value.clone()),
                Err(Error::Stale) => {}
                Err(error) => return Err(error),
            }
        }
        *cached = None;
        let Captured {
            view,
            snapshot,
            owners,
            accepted_revision,
            ..
        } = store.capture_accepted();
        let count = owners.iter().try_fold(0usize, |count, entry| {
            count
                .checked_add(entry.transaction.inputs().len())
                .ok_or(Error::Full("input snapshot count".into()))
        })?;
        let mut inputs = HashSet::new();
        inputs
            .try_reserve(count)
            .map_err(|_| Error::Full("input snapshot allocation".into()))?;
        for entry in owners {
            inputs.extend(
                entry
                    .transaction
                    .input_pts_iter()
                    .map(|point| crate::util::compact_packed(&point)),
            );
        }
        let value = crate::TxPoolInputSnapshot {
            tip_hash: snapshot.tip_hash(),
            inputs: Arc::new(inputs),
        };
        *cached = Some(CachedInputs {
            view,
            accepted_revision,
            value: value.clone(),
        });
        Ok(value)
    }
}

pub(crate) type AcceptedTransactionsWithCycles = Vec<(TransactionView, Cycle)>;

fn public_status(value: Status) -> TxStatus {
    match value {
        Status::Pending | Status::Gap => TxStatus::Pending,
        Status::Proposed => TxStatus::Proposed,
    }
}

pub(super) fn transaction_status(
    store: &Store,
    hash: &Byte32,
) -> Option<(TxStatus, Option<Cycle>)> {
    let (snapshot, entry) = store.point(hash);
    let entry = entry?;
    let value = entry.accepted()?;
    Some((
        public_status(value.status(&snapshot)),
        Some(value.scripts.cycles()),
    ))
}

fn live_transaction(entry: &Entry, snapshot: &Snapshot) -> Option<TransactionWithStatus> {
    let value = entry.accepted()?;
    Some(TransactionWithStatus {
        transaction: Some(entry.transaction.as_ref().clone()),
        tx_status: public_status(value.status(snapshot)),
        cycles: Some(value.scripts.cycles()),
        fee: Some(value.fee),
        min_replace_fee: None,
        time_added_to_pool: Some(value.timestamp),
    })
}

pub(super) fn transaction(
    store: &Store,
    hash: &Byte32,
    config: &TxPoolConfig,
) -> Result<Option<TransactionWithStatus>, Error> {
    let (snapshot, entry) = store.point(hash);
    let Some(entry) = entry else { return Ok(None) };
    if entry
        .accepted()
        .is_some_and(|value| value.status(&snapshot) != Status::Proposed)
        && config.min_rbf_rate > config.min_fee_rate
    {
        return transaction_with_replacement_fee(store, hash, config);
    }
    Ok(live_transaction(&entry, &snapshot))
}

fn transaction_with_replacement_fee(
    store: &Store,
    hash: &Byte32,
    config: &TxPoolConfig,
) -> Result<Option<TransactionWithStatus>, Error> {
    // A chain update can return the point-read target to resolution. Only
    // owners still accepted in this cut can supply public status and fee.
    if let Some((snapshot, entry)) = store.capture_leaf(hash) {
        return transaction_from_descendants(&snapshot, entry.as_slice(), config);
    }
    // Both the complete capture and its aggregation can grow with the pool.
    crate::util::block_offload(|| {
        let (snapshot, owners) = store.capture_descendants(hash)?;
        transaction_from_descendants(&snapshot, &owners, config)
    })
}

fn transaction_from_descendants(
    snapshot: &Snapshot,
    owners: &[Arc<Entry>],
    config: &TxPoolConfig,
) -> Result<Option<TransactionWithStatus>, Error> {
    let Some(entry) = owners.first() else {
        return Ok(None);
    };
    let Some(mut result) = live_transaction(entry, snapshot) else {
        return Ok(None);
    };
    if let Some(value) = entry.accepted()
        && value.status(snapshot) != Status::Proposed
    {
        let increment = config.min_rbf_rate.fee(value.size as u64);
        // These owners belong to one accepted cut, so their count, bytes and
        // cycles also fit the accepted account while we sum their fees.
        let descendants = owners.iter().try_fold(Aggregate::default(), |sum, entry| {
            let value = entry.accepted().ok_or(Error::Stale)?;
            sum.add(Aggregate::one(value))
        })?;
        result.min_replace_fee = descendants.fee().safe_add(increment).ok();
    }
    Ok(Some(result))
}

pub(super) fn summary(store: &Store, config: &TxPoolConfig) -> Result<TxPoolInfo, Error> {
    let summary = store.capture_summary();
    Ok(TxPoolInfo {
        tip_hash: summary.snapshot.tip_hash(),
        tip_number: summary.snapshot.tip_number(),
        pending_size: summary
            .accepted
            .items
            .checked_sub(summary.proposed)
            .ok_or(Error::Fault("summary proposal count"))?,
        proposed_size: summary.proposed,
        orphan_size: summary.waiting,
        total_tx_size: summary.accepted.serialized,
        total_tx_cycles: summary.accepted.cycles,
        min_fee_rate: config.min_fee_rate,
        min_rbf_rate: config.min_rbf_rate,
        last_txs_updated_at: summary.last_updated,
        tx_size_limit: TRANSACTION_SIZE_LIMIT,
        max_tx_pool_size: config.max_tx_pool_size as u64,
        verify_queue_size: summary.queued,
    })
}

pub(super) fn ids(store: &Store) -> TxPoolIds {
    let Captured {
        snapshot, owners, ..
    } = store.capture_accepted();
    let mut result = TxPoolIds {
        pending: Vec::new(),
        proposed: Vec::new(),
    };
    for entry in owners {
        if entry
            .accepted()
            .is_some_and(|value| value.status(&snapshot) == Status::Proposed)
        {
            result.proposed.push(entry.hash());
        } else {
            result.pending.push(entry.hash());
        }
    }
    result.pending.sort_unstable();
    result.proposed.sort_unstable();
    result
}

pub(super) fn entry_info(store: &Store, config: &TxPoolConfig) -> Result<TxPoolEntryInfo, Error> {
    let Captured {
        snapshot, owners, ..
    } = store.capture_all();
    let mut result = TxPoolEntryInfo {
        pending: HashMap::new(),
        proposed: HashMap::new(),
        conflicted: Vec::new(),
    };
    let accepted = owners.iter().filter(|owner| owner.accepted().is_some());
    let totals = membership::aggregates(accepted, config.max_ancestors_count)?;
    for entry in owners {
        let Some(value) = entry.accepted() else {
            if matches!(entry.phase, Phase::Replaced(_)) {
                result.conflicted.push(entry.hash());
            }
            continue;
        };
        let hash = entry.hash();
        let NeighborhoodTotals {
            ancestors,
            descendants,
        } = totals.get(&hash_key(&hash)).ok_or(Error::Stale)?;
        let info = TxEntryInfo {
            cycles: value.scripts.cycles(),
            size: value.size as u64,
            fee: value.fee,
            ancestors_size: ancestors.bytes as u64,
            ancestors_cycles: ancestors.cycles,
            descendants_size: descendants.bytes as u64,
            descendants_cycles: descendants.cycles,
            ancestors_count: ancestors.count as u64,
            timestamp: value.timestamp,
        };
        if value.status(&snapshot) == Status::Proposed {
            result.proposed.insert(hash, info);
        } else {
            result.pending.insert(hash, info);
        }
    }
    result.conflicted.sort_unstable();
    Ok(result)
}

pub(super) fn detail(
    store: &Store,
    hash: &Byte32,
    config: &TxPoolConfig,
) -> Result<PoolTxDetailInfo, Error> {
    store.ranked_detail(hash, config.max_ancestors_count)
}

pub(super) fn score(
    value: &super::model::Accepted,
    ancestors: membership::Aggregate,
) -> Result<AncestorsScoreSortKey, Error> {
    Ok(AncestorsScoreSortKey {
        fee: value.fee,
        weight: get_transaction_weight(value.size, value.scripts.cycles()),
        ancestors_fee: Capacity::shannons(
            u64::try_from(ancestors.fee).map_err(|_| Error::Full("query fee".into()))?,
        ),
        ancestors_weight: get_transaction_weight(ancestors.bytes, ancestors.cycles),
    })
}

pub(super) fn live_cell(store: &Store, point: &OutPoint, with_data: bool) -> CellStatus {
    let (snapshot, overlay) = store.live_cell(point, with_data);
    if let Some(cell) = overlay {
        return cell;
    }
    match snapshot.cell(point, false) {
        CellStatus::Live(mut cell) => {
            if with_data && let Some((data, hash)) = snapshot.get_cell_data(point) {
                cell.mem_cell_data = Some(data);
                cell.mem_cell_data_hash = Some(hash);
            }
            CellStatus::Live(cell)
        }
        CellStatus::Dead | CellStatus::Unknown => CellStatus::Unknown,
    }
}

pub(super) fn compact_transactions(
    store: &Store,
    ids: &[ProposalShortId],
) -> HashMap<ProposalShortId, TransactionView> {
    let (snapshot, live, committed) = store.compact_lookup(ids);
    let mut result: HashMap<_, _> = live
        .into_iter()
        .map(|(id, entry)| (id, entry.transaction.as_ref().clone()))
        .collect();
    for (id, hash) in committed {
        let hash = Byte32::from(&hash);
        if let Some((transaction, _)) = snapshot.get_transaction(&hash)
            && transaction.hash_ref() == &hash
        {
            result.insert(id, transaction);
        }
    }
    result
}

pub(super) fn accepted_with_cycles(
    store: &Store,
    ids: &[Byte32],
) -> AcceptedTransactionsWithCycles {
    store
        .points(ids)
        .into_iter()
        .filter_map(|entry| {
            entry
                .accepted()
                .map(|value| (entry.transaction.as_ref().clone(), value.scripts.cycles()))
        })
        .collect()
}

pub(super) fn fee_samples(store: &Store) -> Vec<FeeSample> {
    store
        .capture_accepted()
        .owners
        .into_iter()
        .filter_map(|entry| {
            entry
                .accepted()
                .map(|value| FeeSample::new(value.size, value.scripts.cycles(), value.fee))
        })
        .collect()
}

pub(super) fn estimate_fee(
    store: &Store,
    config: &TxPoolConfig,
    target: u64,
) -> Result<FeeRate, Error> {
    if !(crate::constants::MIN_ESTIMATE_TARGET..=crate::constants::MAX_ESTIMATE_TARGET)
        .contains(&target)
    {
        return Err(Error::Full("invalid fee estimate target".into()));
    }
    let Captured {
        snapshot, owners, ..
    } = store.capture_accepted();
    let selection = Selection::new(&owners, &snapshot, config.max_ancestors_count)?;
    let mut remaining = target
        .saturating_sub(snapshot.consensus().tx_proposal_window().closest())
        .max(1);
    let mut bytes = 0usize;
    let mut cycles = 0u64;
    for candidate in selection.candidates_by_score() {
        let value = candidate.accepted;
        bytes = bytes
            .checked_add(value.size)
            .ok_or(Error::Full("fee estimate bytes".into()))?;
        cycles = cycles
            .checked_add(value.scripts.cycles())
            .ok_or(Error::Full("fee estimate cycles".into()))?;
        // Approximate each block boundary with the crossing candidate's rate;
        // unlike packing, this includes a candidate that would not fit.
        if bytes >= snapshot.consensus().max_block_bytes() as usize
            || cycles >= snapshot.consensus().max_block_cycles()
        {
            remaining = remaining.saturating_sub(1);
            if remaining == 0 {
                return Ok(FeeRate::calculate(
                    value.fee,
                    get_transaction_weight(value.size, value.scripts.cycles()),
                ));
            }
            bytes = value.size;
            cycles = value.scripts.cycles();
        }
    }
    Ok(config.min_fee_rate)
}

/// Coherent pool counters captured under one short read guard.
pub(super) struct Summary {
    pub(super) snapshot: Arc<Snapshot>,
    pub(super) accepted: Amount,
    pub(super) waiting: usize,
    pub(super) gap: usize,
    pub(super) proposed: usize,
    pub(super) queued: usize,
    pub(super) last_updated: u64,
}

struct DetailStats {
    timestamp: u64,
    status: Status,
    pending_count: usize,
    proposed_count: usize,
    descendants_count: usize,
    ancestors_count: usize,
    target: Priority,
}

impl DetailStats {
    fn into_info(self, ranks: &RankedPriorities) -> PoolTxDetailInfo {
        let rank_in_pending = if self.status == Status::Proposed {
            0
        } else {
            ranks.ahead(&self.target).saturating_add(1)
        };
        PoolTxDetailInfo {
            timestamp: self.timestamp,
            entry_status: match self.status {
                Status::Pending => "pending",
                Status::Gap => "gap",
                Status::Proposed => "proposed",
            }
            .to_owned(),
            rank_in_pending,
            pending_count: self.pending_count,
            proposed_count: self.proposed_count,
            descendants_count: self.descendants_count,
            ancestors_count: self.ancestors_count,
            score_sortkey: self.target.score.into(),
        }
    }
}

impl Store {
    pub(super) fn points(&self, hashes: &[Byte32]) -> Vec<Arc<Entry>> {
        let state = self.state_read();
        hashes
            .iter()
            .filter_map(|hash| state.owners.get(&hash_key(hash)).cloned())
            .collect()
    }

    /// Filter IDs already owned by any pool phase at one coherent cut. Keep
    /// unknown and committed-only IDs in their input order, including repeats.
    pub(super) fn filter_fresh_proposals(
        &self,
        mut ids: Vec<ProposalShortId>,
    ) -> Vec<ProposalShortId> {
        let state = self.state_read();
        ids.retain(|id| state.proposal_owner(&id.unpack()).is_none());
        ids
    }

    #[expect(
        clippy::type_complexity,
        reason = "One coherent lookup returns live owners and committed hashes with their chain snapshot."
    )]
    pub(super) fn compact_lookup(
        &self,
        ids: &[ProposalShortId],
    ) -> (
        Arc<Snapshot>,
        Vec<(ProposalShortId, Arc<Entry>)>,
        Vec<(ProposalShortId, [u8; 32])>,
    ) {
        let state = self.state_read();
        let mut live = Vec::with_capacity(ids.len());
        let mut committed = Vec::with_capacity(ids.len());
        for id in ids {
            let key = id.unpack();
            if let Some(entry) = state.proposal_owner(&key) {
                live.push((id.clone(), Arc::clone(entry)));
            } else if let Some(hash) = state.committed.peek(&key) {
                committed.push((id.clone(), *hash));
            }
        }
        (Arc::clone(&state.snapshot), live, committed)
    }

    pub(super) fn live_cell(
        &self,
        point: &OutPoint,
        with_data: bool,
    ) -> (Arc<Snapshot>, Option<CellStatus>) {
        let key = OutPointKey::from_out_point(point);
        let (snapshot, spent, owner) = {
            let state = self.state_read();
            let snapshot = Arc::clone(&state.snapshot);
            if state.spenders.contains_key(&key) {
                (snapshot, true, None)
            } else {
                let owner = state
                    .owners
                    .get(key.tx_hash())
                    .filter(|entry| entry.accepted().is_some())
                    .cloned();
                (snapshot, false, owner)
            }
        };
        if spent {
            return (snapshot, Some(CellStatus::Unknown));
        }
        let Some(owner) = owner else {
            return (snapshot, None);
        };
        let Some((output, data)) = owner.transaction.output_with_data(key.index() as usize) else {
            return (snapshot, None);
        };
        let output = CellOutput::new_unchecked(Bytes::copy_from_slice(output.as_slice()));
        (
            snapshot,
            Some(CellStatus::Live(CellMeta {
                cell_output: output,
                out_point: compact_packed(point),
                data_bytes: data.len() as u64,
                mem_cell_data: with_data.then(|| Bytes::copy_from_slice(&data)),
                mem_cell_data_hash: with_data.then(|| CellOutput::calc_data_hash(&data)),
                transaction_info: None,
            })),
        )
    }

    pub(super) fn capture_leaf(
        &self,
        hash: &Byte32,
    ) -> Option<(Arc<Snapshot>, Option<Arc<Entry>>)> {
        let state = self.state_read();
        let leaf = state.is_leaf_accepted(&hash_key(hash))?;
        Some((Arc::clone(&state.snapshot), leaf))
    }

    pub(super) fn capture_descendants(
        &self,
        hash: &Byte32,
    ) -> Result<(Arc<Snapshot>, Vec<Arc<Entry>>), Error> {
        let state = self.state_read();
        let owners = state.capture_descendants(&hash_key(hash))?;
        Ok((Arc::clone(&state.snapshot), owners))
    }

    pub(super) fn capture_summary(&self) -> Summary {
        let state = self.state_read();
        Summary {
            snapshot: Arc::clone(&state.snapshot),
            accepted: state.usage.accepted,
            waiting: state.waiting_owners.len(),
            gap: state.gap_count,
            proposed: state.proposed_count,
            queued: self.queued_len(),
            last_updated: state.last_updated(),
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Counts and closures are bounded by the accepted population."
    )]
    pub(super) fn ranked_detail(
        &self,
        hash: &Byte32,
        limit: usize,
    ) -> Result<PoolTxDetailInfo, Error> {
        let key = hash_key(hash);
        let (view, accepted_revision, generation, snapshot, owners, stats) = {
            let state = self.state_read();
            let Some(owner) = state.owners.get(&key) else {
                return Ok(PoolTxDetailInfo::with_unknown());
            };
            let Some(value) = owner.accepted() else {
                return Ok(PoolTxDetailInfo::with_unknown());
            };
            let descendants_count = state.descendant_count(&key)?;
            let status = value.status(&state.snapshot);
            let ancestors = if value.parents.is_empty() {
                if limit == 0 {
                    return Err(crate::error::Reject::ExceededMaximumAncestorsCount.into());
                }
                Aggregate::one(value)
            } else {
                let mut ancestors = Aggregate::default();
                membership::walk_ancestors(
                    owner,
                    limit,
                    |parent| state.owners.get(&hash_key(parent)).map(Arc::as_ref),
                    |entry| {
                        ancestors = ancestors.add(Aggregate::one(entry))?;
                        Ok(())
                    },
                )?;
                ancestors
            };
            let proposed_count = state.proposed_count;
            let stats = DetailStats {
                timestamp: value.timestamp,
                status,
                pending_count: state.usage.accepted.items.saturating_sub(proposed_count),
                proposed_count,
                descendants_count,
                ancestors_count: ancestors.count - 1,
                target: state.priority_for(owner, ancestors)?,
            };
            if state.pending_ranks.is_current(state.rank_generation, limit) {
                return Ok(stats.into_info(&state.pending_ranks));
            }
            let owners: Vec<Arc<Entry>> = state
                .owners
                .values()
                .filter(|entry| entry.accepted().is_some())
                .cloned()
                .collect();
            (
                state.view,
                state.accepted_revision,
                state.rank_generation,
                Arc::clone(&state.snapshot),
                owners,
                stats,
            )
        };
        let ranks = RankedPriorities::rebuild(&owners, &snapshot, generation, limit)?;
        let info = stats.into_info(&ranks);
        self.install_pending_ranks(view, accepted_revision, generation, ranks);
        Ok(info)
    }
}

#[cfg(test)]
#[path = "tests/query.rs"]
mod tests;
