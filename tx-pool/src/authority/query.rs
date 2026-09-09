//! Public values derived from coherent immutable owners. Full projections are
//! called only while a service read slot owns their bounded scratch lifetime.
use super::{
    membership::{self, Members},
    model::{Entry, Error, Phase, Status},
    packing::Selection,
    store::Store,
};
use crate::component::sort_key::AncestorsScoreSortKey;
use ckb_app_config::TxPoolConfig;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        Capacity, Cycle, FeeRate, TransactionView,
        cell::{CellProvider, CellStatus},
        tx_pool::{
            PoolTxDetailInfo, TRANSACTION_SIZE_LIMIT, TransactionWithStatus, TxEntryInfo,
            TxPoolEntryInfo, TxPoolIds, TxPoolInfo, TxStatus, get_transaction_weight,
        },
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::collections::{BTreeMap, HashMap};

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
    Some((public_status(value.status(&snapshot)), Some(value.cycles)))
}
fn live_transaction(entry: &Entry, snapshot: &Snapshot) -> Option<TransactionWithStatus> {
    let value = entry.accepted()?;
    Some(TransactionWithStatus {
        transaction: Some(entry.transaction.as_ref().clone()),
        tx_status: public_status(value.status(snapshot)),
        cycles: Some(value.cycles),
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
    let (_, snapshot, owners, _) = store.capture(true);
    let Some(entry) = owners.iter().find(|entry| entry.hash() == *hash) else {
        return Ok(None);
    };
    let Some(mut result) = live_transaction(entry, &snapshot) else {
        return Ok(None);
    };
    if let Some(value) = entry.accepted()
        && value.status(&snapshot) != Status::Proposed
    {
        let increment = config.min_rbf_rate.fee(value.size as u64);
        let members: Members = owners
            .into_iter()
            .map(|entry| (entry.hash(), entry))
            .collect();
        let descendants = membership::descendant_hashes(
            &membership::children(&members),
            [hash.clone()],
            members.len(),
        )?;
        result.min_replace_fee = membership::aggregate(&members, &descendants)?
            .fee()
            .safe_add(increment)
            .ok();
    }
    Ok(Some(result))
}
#[expect(
    clippy::arithmetic_side_effects,
    reason = "Each counter advances once per captured owner and cannot exceed the bounded Vec length."
)]
pub(super) fn summary(store: &Store, config: &TxPoolConfig) -> Result<TxPoolInfo, Error> {
    let (snapshot, owners, queued) = store.capture_summary();
    let mut summary = TxPoolInfo {
        tip_hash: snapshot.tip_hash(),
        tip_number: snapshot.tip_number(),
        pending_size: 0,
        proposed_size: 0,
        orphan_size: 0,
        total_tx_size: 0,
        total_tx_cycles: 0,
        min_fee_rate: config.min_fee_rate,
        min_rbf_rate: config.min_rbf_rate,
        last_txs_updated_at: 0,
        tx_size_limit: TRANSACTION_SIZE_LIMIT,
        max_tx_pool_size: config.max_tx_pool_size as u64,
        verify_queue_size: queued,
    };
    for owner in owners {
        match &owner.phase {
            Phase::Accepted(value) => {
                if value.status(&snapshot) == Status::Proposed {
                    summary.proposed_size += 1;
                } else {
                    summary.pending_size += 1;
                }
                summary.total_tx_size = summary
                    .total_tx_size
                    .checked_add(value.size)
                    .ok_or(Error::Full("query bytes".into()))?;
                summary.total_tx_cycles = summary
                    .total_tx_cycles
                    .checked_add(value.cycles)
                    .ok_or(Error::Full("query cycles".into()))?;
                summary.last_txs_updated_at = summary.last_txs_updated_at.max(value.timestamp);
            }
            Phase::Waiting(_) => summary.orphan_size += 1,
            _ => {}
        }
    }
    Ok(summary)
}
pub(super) fn ids(store: &Store) -> TxPoolIds {
    let (_, snapshot, owners, _) = store.capture(true);
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
    let (_, snapshot, owners, _) = store.capture(false);
    let mut result = TxPoolEntryInfo {
        pending: HashMap::new(),
        proposed: HashMap::new(),
        conflicted: Vec::new(),
    };
    let mut members = Members::new();
    for entry in owners {
        if entry.accepted().is_some() {
            members.insert(entry.hash(), entry);
        } else if matches!(entry.phase, Phase::Replaced { .. }) {
            result.conflicted.push(entry.hash());
        }
    }
    let totals = membership::aggregates(&members, config.max_ancestors_count)?;
    for (hash, entry) in members {
        let value = entry.accepted().ok_or(Error::Stale)?;
        let (ancestors, descendants) = totals.get(&hash).ok_or(Error::Stale)?;
        let info = TxEntryInfo {
            cycles: value.cycles,
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
#[expect(
    clippy::arithmetic_side_effects,
    reason = "Counts and one-based rank cannot exceed the bounded captured membership length."
)]
pub(super) fn detail(
    store: &Store,
    hash: &Byte32,
    config: &TxPoolConfig,
) -> Result<PoolTxDetailInfo, Error> {
    let (_, snapshot, owners, _) = store.capture(true);
    let members: Members = owners
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let Some(entry) = members.get(hash) else {
        return Ok(PoolTxDetailInfo::with_unknown());
    };
    let value = entry.accepted().ok_or(Error::Stale)?;
    let totals = membership::aggregates(&members, config.max_ancestors_count)?;
    let (ancestors, descendants) = totals.get(hash).ok_or(Error::Stale)?;
    let target_score = score(value, *ancestors)?;
    let proposed = value.status(&snapshot) == Status::Proposed;
    let mut pending_count = 0usize;
    let mut proposed_count = 0usize;
    let mut rank_in_pending = usize::from(!proposed);
    for (candidate_hash, candidate) in &members {
        let candidate_value = candidate.accepted().ok_or(Error::Stale)?;
        if candidate_value.status(&snapshot) == Status::Proposed {
            proposed_count += 1;
            continue;
        }
        pending_count += 1;
        let (candidate_ancestors, _) = totals.get(candidate_hash).ok_or(Error::Stale)?;
        let candidate_score = score(candidate_value, *candidate_ancestors)?;
        if !proposed
            && target_score
                .cmp(&candidate_score)
                .then_with(|| candidate.arrival.cmp(&entry.arrival))
                .then_with(|| candidate_hash.cmp(hash))
                .is_lt()
        {
            rank_in_pending += 1;
        }
    }
    Ok(PoolTxDetailInfo {
        timestamp: value.timestamp,
        entry_status: match value.status(&snapshot) {
            Status::Pending => "pending",
            Status::Gap => "gap",
            Status::Proposed => "proposed",
        }
        .to_owned(),
        rank_in_pending,
        pending_count,
        proposed_count,
        descendants_count: descendants.count.checked_sub(1).ok_or(Error::Stale)?,
        ancestors_count: ancestors.count.checked_sub(1).ok_or(Error::Stale)?,
        score_sortkey: target_score.into(),
    })
}
fn score(
    value: &super::model::Accepted,
    ancestors: membership::Aggregate,
) -> Result<AncestorsScoreSortKey, Error> {
    Ok(AncestorsScoreSortKey {
        fee: value.fee,
        weight: get_transaction_weight(value.size, value.cycles),
        ancestors_fee: Capacity::shannons(
            u64::try_from(ancestors.fee).map_err(|_| Error::Full("query fee".into()))?,
        ),
        ancestors_weight: get_transaction_weight(ancestors.bytes, ancestors.cycles),
    })
}
pub(super) fn live_cell(store: &Store, point: &OutPoint, with_data: bool) -> CellStatus {
    let (snapshot, overlay) = store.live_cell(point);
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
        if let Some((transaction, _)) = snapshot.get_transaction(&hash)
            && transaction.hash() == hash
            && transaction.proposal_short_id() == id
        {
            result.insert(id, transaction);
        }
    }
    result
}
pub(super) fn fresh_proposals(store: &Store, ids: Vec<ProposalShortId>) -> Vec<ProposalShortId> {
    let (_, live, _) = store.compact_lookup(&ids);
    let present: BTreeMap<_, _> = live.into_iter().collect();
    ids.into_iter()
        .filter(|id| !present.contains_key(id))
        .collect()
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
                .map(|value| (entry.transaction.as_ref().clone(), value.cycles))
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
    let (_, snapshot, owners, _) = store.capture(true);
    let selection = Selection::new(owners, &snapshot, config.max_ancestors_count)?;
    let mut remaining = target
        .saturating_sub(snapshot.consensus().tx_proposal_window().closest())
        .max(1);
    let mut bytes = 0usize;
    let mut cycles = 0u64;
    for candidate in selection.candidates() {
        let value = &candidate.accepted;
        bytes = bytes
            .checked_add(value.size)
            .ok_or(Error::Full("fee estimate bytes".into()))?;
        cycles = cycles
            .checked_add(value.cycles)
            .ok_or(Error::Full("fee estimate cycles".into()))?;
        if bytes >= snapshot.consensus().max_block_bytes() as usize
            || cycles >= snapshot.consensus().max_block_cycles()
        {
            remaining = remaining.saturating_sub(1);
            if remaining == 0 {
                return Ok(FeeRate::calculate(
                    value.fee,
                    get_transaction_weight(value.size, value.cycles),
                ));
            }
            bytes = value.size;
            cycles = value.cycles;
        }
    }
    Ok(config.min_fee_rate)
}

#[cfg(test)]
#[path = "tests/query.rs"]
mod tests;
