//! Owned production query receipts compiled from one authority read cut.
//!
//! These values contain no authority references or guards. Service adapters
//! may perform storage lookup, DTO conversion, sorting and destruction after
//! capture without extending the membership critical section.

use super::{
    read::{
        AuthorityReadEntry, AuthorityReadError, AuthorityReadSummary, AuthorityReadView,
        AuthorityRpcStatus, ParentFirstPersistence, PersistenceReadReceipt,
    },
    state::{AcceptedStatus, RawTxHash},
};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        BlockNumber, Capacity, Cycle, FeeRate, TransactionView,
        cell::{CellMetaBuilder, CellProvider, CellStatus},
        tx_pool::{PoolTxDetailInfo, TxEntryInfo, TxPoolEntryInfo, TxPoolIds},
    },
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::Unpack,
};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityQueryError {
    Allocation,
    Arithmetic,
    Projection,
    AcceptedCycle,
    RecoveryCycle,
}

impl From<AuthorityReadError> for AuthorityQueryError {
    fn from(error: AuthorityReadError) -> Self {
        match error {
            AuthorityReadError::Allocation => Self::Allocation,
            AuthorityReadError::Arithmetic => Self::Arithmetic,
            AuthorityReadError::Projection => Self::Projection,
            AuthorityReadError::AcceptedCycle => Self::AcceptedCycle,
            AuthorityReadError::RecoveryCycle => Self::RecoveryCycle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicPoolStatus {
    Pending,
    Proposed,
}

impl From<AuthorityRpcStatus> for PublicPoolStatus {
    fn from(status: AuthorityRpcStatus) -> Self {
        match status {
            AuthorityRpcStatus::Pending => Self::Pending,
            AuthorityRpcStatus::Proposed => Self::Proposed,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AuthorityTransaction {
    pub(crate) transaction: Arc<TransactionView>,
    pub(crate) status: PublicPoolStatus,
    pub(crate) cycles: Option<Cycle>,
    pub(crate) fee: Option<Capacity>,
    pub(crate) accepted_at: Option<u64>,
    pub(crate) min_replace_fee: Option<Capacity>,
}

#[derive(Clone, Debug)]
pub(crate) enum AuthorityTransactionLookup {
    Live(AuthorityTransaction),
    RecentRejectFallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthorityTransactionStatus {
    pub(crate) status: PublicPoolStatus,
    pub(crate) cycles: Option<Cycle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityTransactionStatusLookup {
    Live(AuthorityTransactionStatus),
    RecentRejectFallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthorityPoolSummary {
    pub(crate) tip_hash: Byte32,
    pub(crate) tip_number: BlockNumber,
    pub(crate) pending_size: usize,
    pub(crate) proposed_size: usize,
    pub(crate) orphan_size: usize,
    pub(crate) total_tx_size: usize,
    pub(crate) total_tx_cycles: Cycle,
    pub(crate) last_txs_updated_at: u64,
    pub(crate) verify_queue_size: usize,
}

impl AuthorityPoolSummary {
    pub(super) fn capture(
        snapshot: &Snapshot,
        summary: AuthorityReadSummary,
    ) -> Result<Self, AuthorityReadError> {
        let pending_size = summary
            .accepted_pending
            .checked_add(summary.accepted_gap)
            .ok_or(AuthorityReadError::Arithmetic)?;
        let tip = snapshot.tip_header();
        Ok(Self {
            tip_hash: tip.hash(),
            tip_number: tip.number(),
            pending_size,
            proposed_size: summary.accepted_proposed,
            orphan_size: summary.waiting_missing,
            total_tx_size: summary.accepted_resources.serialized_bytes,
            total_tx_cycles: summary.accepted_resources.cycles,
            last_txs_updated_at: summary.latest_accepted_at.map_or(0, |at| at.0),
            verify_queue_size: summary.verify_queued,
        })
    }
}

pub(super) fn transaction_lookup(
    view: &AuthorityReadView<'_>,
    snapshot: &Snapshot,
    hash: &RawTxHash,
) -> Result<AuthorityTransactionLookup, AuthorityReadError> {
    let Some(entry) = view.entry_by_raw(hash) else {
        return Ok(AuthorityTransactionLookup::RecentRejectFallback);
    };
    let Some(status) = transaction_status(&entry, snapshot) else {
        return Ok(AuthorityTransactionLookup::RecentRejectFallback);
    };
    let min_replace_fee = match entry.state() {
        super::read::AuthorityReadState::Accepted(
            AcceptedStatus::Pending | AcceptedStatus::Gap,
        ) => minimum_replacement_fee(view, hash)?,
        super::read::AuthorityReadState::Accepted(AcceptedStatus::Proposed)
        | super::read::AuthorityReadState::PreAccepted(_)
        | super::read::AuthorityReadState::ReplacementHistory => None,
    };
    Ok(AuthorityTransactionLookup::Live(AuthorityTransaction {
        transaction: Arc::clone(entry.transaction()),
        status: status.status,
        cycles: status.cycles,
        fee: entry.fee(),
        accepted_at: entry.accepted_at().map(|accepted_at| accepted_at.0),
        min_replace_fee,
    }))
}

/// Compile only the public status projection needed by the lightweight RPC.
///
/// Keeping this distinct from [`transaction_lookup`] is a correctness
/// boundary: optional detail arithmetic such as the minimum replacement fee
/// cannot turn a coherent status query into an authority-generation fault.
pub(super) fn transaction_status_lookup(
    view: &AuthorityReadView<'_>,
    snapshot: &Snapshot,
    hash: &RawTxHash,
) -> AuthorityTransactionStatusLookup {
    let Some(entry) = view.entry_by_raw(hash) else {
        return AuthorityTransactionStatusLookup::RecentRejectFallback;
    };
    let Some(status) = transaction_status(&entry, snapshot) else {
        return AuthorityTransactionStatusLookup::RecentRejectFallback;
    };
    AuthorityTransactionStatusLookup::Live(status)
}

fn transaction_status(
    entry: &AuthorityReadEntry<'_>,
    snapshot: &Snapshot,
) -> Option<AuthorityTransactionStatus> {
    Some(AuthorityTransactionStatus {
        status: entry.rpc_status(snapshot)?.into(),
        cycles: entry.transaction_status_cycles(),
    })
}

fn minimum_replacement_fee(
    view: &AuthorityReadView<'_>,
    hash: &RawTxHash,
) -> Result<Option<Capacity>, AuthorityReadError> {
    let Some(rate) = view.minimum_replacement_rate() else {
        return Ok(None);
    };
    let Some(accepted) = view.accepted_entry_by_raw(hash)? else {
        return Ok(None);
    };
    let Ok(size) = u64::try_from(accepted.entry().proof.metrics().cost.serialized_bytes) else {
        return Ok(None);
    };
    Ok(accepted.descendant().fee.safe_add(rate.fee(size)).ok())
}

pub(super) fn pool_ids(view: &AuthorityReadView<'_>) -> Result<TxPoolIds, AuthorityReadError> {
    let ids = view.pool_ids()?;
    let mut pending = Vec::new();
    let mut proposed = Vec::new();
    pending
        .try_reserve(ids.pending.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    proposed
        .try_reserve(ids.proposed.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    pending.extend(ids.pending.into_iter().map(|hash| hash.0));
    proposed.extend(ids.proposed.into_iter().map(|hash| hash.0));
    Ok(TxPoolIds { pending, proposed })
}

pub(super) fn all_entry_info(
    view: &AuthorityReadView<'_>,
) -> Result<TxPoolEntryInfo, AuthorityReadError> {
    let summary = view.summary()?;
    let accepted_count = summary
        .accepted_pending
        .checked_add(summary.accepted_gap)
        .and_then(|count| count.checked_add(summary.accepted_proposed))
        .ok_or(AuthorityReadError::Arithmetic)?;
    let mut pending = HashMap::new();
    let mut proposed = HashMap::new();
    let pending_count = summary
        .accepted_pending
        .checked_add(summary.accepted_gap)
        .ok_or(AuthorityReadError::Arithmetic)?;
    pending
        .try_reserve(pending_count)
        .map_err(|_| AuthorityReadError::Allocation)?;
    proposed
        .try_reserve(summary.accepted_proposed)
        .map_err(|_| AuthorityReadError::Allocation)?;
    for order in view.accepted_order().rev() {
        let accepted = view.accepted_entry_for_order(order)?;
        let entry = accepted.entry();
        let info = entry_info(&accepted)?;
        match entry.status() {
            AcceptedStatus::Pending | AcceptedStatus::Gap => {
                pending.insert(order.hash().0.clone(), info);
            }
            AcceptedStatus::Proposed => {
                proposed.insert(order.hash().0.clone(), info);
            }
        }
    }
    let projected_count = pending
        .len()
        .checked_add(proposed.len())
        .ok_or(AuthorityReadError::Arithmetic)?;
    if projected_count != accepted_count {
        return Err(AuthorityReadError::Projection);
    }
    let history = view.replacement_history_hashes()?;
    let mut conflicted = Vec::new();
    conflicted
        .try_reserve(history.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    conflicted.extend(history.into_iter().map(|hash| hash.0));
    Ok(TxPoolEntryInfo {
        pending,
        proposed,
        conflicted,
    })
}

fn entry_info(
    accepted: &super::read::AcceptedReadEntry<'_>,
) -> Result<TxEntryInfo, AuthorityReadError> {
    let entry = accepted.entry();
    let cost = entry.proof.metrics().cost;
    let ancestor = accepted.ancestor();
    let descendant = accepted.descendant();
    Ok(TxEntryInfo {
        cycles: cost.cycles,
        size: u64::try_from(cost.serialized_bytes).map_err(|_| AuthorityReadError::Arithmetic)?,
        fee: entry.proof.metrics().fee,
        ancestors_size: u64::try_from(ancestor.serialized_bytes)
            .map_err(|_| AuthorityReadError::Arithmetic)?,
        ancestors_cycles: ancestor.cycles,
        descendants_size: u64::try_from(descendant.serialized_bytes)
            .map_err(|_| AuthorityReadError::Arithmetic)?,
        descendants_cycles: descendant.cycles,
        ancestors_count: u64::try_from(ancestor.entries)
            .map_err(|_| AuthorityReadError::Arithmetic)?,
        timestamp: entry.accepted_at.0,
    })
}

pub(super) fn pool_detail(
    view: &AuthorityReadView<'_>,
    hash: &RawTxHash,
) -> Result<Option<PoolTxDetailInfo>, AuthorityReadError> {
    let Some(target) = view.accepted_entry_by_raw(hash)? else {
        return Ok(None);
    };
    let summary = view.summary()?;
    let pending_count = summary
        .accepted_pending
        .checked_add(summary.accepted_gap)
        .ok_or(AuthorityReadError::Arithmetic)?;
    let rank_in_pending = if target.entry().status() == AcceptedStatus::Proposed {
        0
    } else {
        let mut rank = None;
        let mut pending_position = 0usize;
        for order in view.accepted_order().rev() {
            let entry = view.accepted_entry_for_order(order)?;
            if !matches!(
                entry.entry().status(),
                AcceptedStatus::Pending | AcceptedStatus::Gap
            ) {
                continue;
            }
            if order.hash() == hash {
                rank = Some(
                    pending_position
                        .checked_add(1)
                        .ok_or(AuthorityReadError::Arithmetic)?,
                );
                break;
            }
            pending_position = pending_position
                .checked_add(1)
                .ok_or(AuthorityReadError::Arithmetic)?;
        }
        rank.ok_or(AuthorityReadError::Projection)?
    };
    let ancestors_count = target
        .ancestor()
        .entries
        .checked_sub(1)
        .ok_or(AuthorityReadError::Projection)?;
    let descendants_count = target
        .descendant()
        .entries
        .checked_sub(1)
        .ok_or(AuthorityReadError::Projection)?;
    let entry_status = match target.entry().status() {
        AcceptedStatus::Pending => "pending",
        AcceptedStatus::Gap => "gap",
        AcceptedStatus::Proposed => "proposed",
    };
    Ok(Some(PoolTxDetailInfo {
        timestamp: target.entry().accepted_at.0,
        entry_status: entry_status.to_owned(),
        rank_in_pending,
        pending_count,
        proposed_count: summary.accepted_proposed,
        descendants_count,
        ancestors_count,
        score_sortkey: target.order().score().clone().into(),
    }))
}

#[derive(Debug)]
pub(crate) struct LiveCellReadReceipt {
    snapshot: Arc<Snapshot>,
    out_point: OutPoint,
    overlay: LiveCellOverlay,
}

#[derive(Debug)]
enum LiveCellOverlay {
    Spent,
    Producer(Arc<TransactionView>),
    Chain,
}

impl LiveCellReadReceipt {
    pub(super) fn capture(
        view: &AuthorityReadView<'_>,
        snapshot: Arc<Snapshot>,
        out_point: OutPoint,
    ) -> Self {
        let overlay = if view.accepted_spends(&out_point) {
            LiveCellOverlay::Spent
        } else {
            let producer = RawTxHash(out_point.tx_hash());
            let output_index: u32 = out_point.index().unpack();
            match view.entry_by_raw(&producer) {
                Some(entry)
                    if matches!(entry.state(), super::read::AuthorityReadState::Accepted(_))
                        && usize::try_from(output_index)
                            .ok()
                            .is_some_and(|index| index < entry.transaction().outputs().len()) =>
                {
                    LiveCellOverlay::Producer(Arc::clone(entry.transaction()))
                }
                Some(_) | None => LiveCellOverlay::Chain,
            }
        };
        Self {
            snapshot,
            out_point,
            overlay,
        }
    }

    pub(crate) fn resolve(self, eager_load: bool) -> CellStatus {
        match self.overlay {
            LiveCellOverlay::Spent => CellStatus::Unknown,
            LiveCellOverlay::Producer(transaction) => {
                let index: u32 = self.out_point.index().unpack();
                let Some(index) = usize::try_from(index).ok() else {
                    return CellStatus::Unknown;
                };
                let Some((output, data)) = transaction.output_with_data(index) else {
                    return CellStatus::Unknown;
                };
                CellStatus::live_cell(
                    CellMetaBuilder::from_cell_output(output, data)
                        .out_point(self.out_point)
                        .build(),
                )
            }
            LiveCellOverlay::Chain => match self.snapshot.cell(&self.out_point, false) {
                CellStatus::Live(mut cell_meta) => {
                    if eager_load
                        && let Some((data, data_hash)) =
                            self.snapshot.get_cell_data(&self.out_point)
                    {
                        cell_meta.mem_cell_data = Some(data);
                        cell_meta.mem_cell_data_hash = Some(data_hash);
                    }
                    CellStatus::live_cell(cell_meta)
                }
                CellStatus::Dead | CellStatus::Unknown => CellStatus::Unknown,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) struct CompactBlockReadReceipt {
    snapshot: Arc<Snapshot>,
    transactions: HashMap<ProposalShortId, TransactionView>,
    committed: Vec<(ProposalShortId, Byte32)>,
}

impl CompactBlockReadReceipt {
    pub(super) fn capture(
        view: &AuthorityReadView<'_>,
        snapshot: Arc<Snapshot>,
        requested: &[ProposalShortId],
        committed: Vec<(ProposalShortId, Byte32)>,
    ) -> Result<Self, AuthorityReadError> {
        let proposals: Vec<_> = requested
            .iter()
            .cloned()
            .map(super::state::ProposalId)
            .collect();
        let captured = view.compact_transactions(&proposals)?;
        let mut transactions = HashMap::new();
        transactions
            .try_reserve(captured.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        transactions.extend(
            captured
                .into_iter()
                .map(|(proposal, transaction)| (proposal.0, transaction.as_ref().clone())),
        );
        Ok(Self {
            snapshot,
            transactions,
            committed,
        })
    }

    pub(crate) fn resolve(
        mut self,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, AuthorityQueryError> {
        self.transactions
            .try_reserve(self.committed.len())
            .map_err(|_| AuthorityQueryError::Allocation)?;
        for (proposal, hash) in self.committed {
            if self.transactions.contains_key(&proposal) {
                continue;
            }
            let Some((transaction, _)) = self.snapshot.get_transaction(&hash) else {
                continue;
            };
            if transaction.hash() != hash || transaction.proposal_short_id() != proposal {
                continue;
            }
            self.transactions.insert(proposal, transaction);
        }
        Ok(self.transactions)
    }
}

pub(super) fn accepted_with_cycles(
    view: &AuthorityReadView<'_>,
    requested: &[ProposalShortId],
) -> Result<HashMap<ProposalShortId, (TransactionView, Cycle)>, AuthorityReadError> {
    let mut result = HashMap::new();
    result
        .try_reserve(requested.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    for proposal in requested {
        let Some(entry) = view.entry_by_proposal(&super::state::ProposalId(proposal.clone()))?
        else {
            continue;
        };
        if !matches!(entry.state(), super::read::AuthorityReadState::Accepted(_)) {
            continue;
        }
        let cycles = entry.cycles().ok_or(AuthorityReadError::Projection)?;
        result.insert(
            proposal.clone(),
            (entry.transaction().as_ref().clone(), cycles),
        );
    }
    Ok(result)
}

pub(super) fn filter_fresh_proposals(
    view: &AuthorityReadView<'_>,
    proposals: &mut Vec<ProposalShortId>,
) -> Result<(), AuthorityReadError> {
    let mut failure = None;
    proposals.retain(|proposal| {
        match view.entry_by_proposal(&super::state::ProposalId(proposal.clone())) {
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(error) => {
                failure = Some(error);
                false
            }
        }
    });
    failure.map_or(Ok(()), Err)
}

#[derive(Debug)]
pub(crate) struct PersistenceReceipt(PersistenceReadReceipt);

impl PersistenceReceipt {
    pub(super) fn capture(view: &AuthorityReadView<'_>) -> Result<Self, AuthorityReadError> {
        view.capture_persistence().map(Self)
    }

    pub(crate) fn into_parent_first(
        self,
    ) -> Result<ParentFirstPersistenceReceipt, AuthorityQueryError> {
        let parent_first = self.0.into_parent_first()?;
        Ok(ParentFirstPersistenceReceipt(parent_first))
    }
}

#[derive(Debug)]
pub(crate) struct ParentFirstPersistenceReceipt(ParentFirstPersistence);

impl ParentFirstPersistenceReceipt {
    pub(crate) fn into_transactions(
        self,
    ) -> (Vec<Arc<TransactionView>>, Vec<Arc<TransactionView>>) {
        self.0.into_transactions()
    }
}

#[cfg(test)]
#[path = "tests/support/query.rs"]
mod test_support;

#[derive(Clone, Copy, Debug)]
struct FeeCandidate {
    fee: Capacity,
    bytes: usize,
    cycles: Cycle,
}

#[derive(Debug)]
pub(crate) struct FeeEstimateReadReceipt {
    closest: BlockNumber,
    max_block_bytes: usize,
    max_block_cycles: Cycle,
    min_fee_rate: FeeRate,
    candidates: Vec<FeeCandidate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FeeEstimateReadError {
    Arithmetic,
    InvalidTarget,
}

impl FeeEstimateReadReceipt {
    pub(super) fn capture(
        view: &AuthorityReadView<'_>,
        snapshot: &Snapshot,
        min_fee_rate: FeeRate,
    ) -> Result<Self, AuthorityReadError> {
        let summary = view.summary()?;
        let mut candidates = Vec::new();
        candidates
            .try_reserve(summary.accepted_resources.entries)
            .map_err(|_| AuthorityReadError::Allocation)?;
        for order in view.accepted_order().rev() {
            let accepted = view.accepted_entry_for_order(order)?;
            let metrics = accepted.entry().proof.metrics();
            candidates.push(FeeCandidate {
                fee: metrics.fee,
                bytes: metrics.cost.serialized_bytes,
                cycles: metrics.cost.cycles,
            });
        }
        if candidates.len() != summary.accepted_resources.entries {
            return Err(AuthorityReadError::Projection);
        }
        Ok(Self {
            closest: snapshot.consensus().tx_proposal_window().closest(),
            max_block_bytes: usize::try_from(snapshot.consensus().max_block_bytes())
                .map_err(|_| AuthorityReadError::Arithmetic)?,
            max_block_cycles: snapshot.consensus().max_block_cycles(),
            min_fee_rate,
            candidates,
        })
    }

    pub(crate) fn estimate(
        self,
        target_to_be_committed: BlockNumber,
    ) -> Result<FeeRate, FeeEstimateReadError> {
        if !(crate::constants::MIN_ESTIMATE_TARGET..=crate::constants::MAX_ESTIMATE_TARGET)
            .contains(&target_to_be_committed)
        {
            return Err(FeeEstimateReadError::InvalidTarget);
        }
        let mut remaining_blocks =
            usize::try_from(target_to_be_committed.saturating_sub(self.closest).max(1))
                .map_err(|_| FeeEstimateReadError::InvalidTarget)?;
        let mut current_block_bytes = 0usize;
        let mut current_block_cycles = 0u64;
        for candidate in self.candidates {
            current_block_bytes = current_block_bytes
                .checked_add(candidate.bytes)
                .ok_or(FeeEstimateReadError::Arithmetic)?;
            current_block_cycles = current_block_cycles
                .checked_add(candidate.cycles)
                .ok_or(FeeEstimateReadError::Arithmetic)?;
            if current_block_bytes >= self.max_block_bytes
                || current_block_cycles >= self.max_block_cycles
            {
                remaining_blocks = match remaining_blocks.checked_sub(1) {
                    Some(0) => {
                        return Ok(FeeRate::calculate(
                            candidate.fee,
                            ckb_types::core::tx_pool::get_transaction_weight(
                                candidate.bytes,
                                candidate.cycles,
                            ),
                        ));
                    }
                    Some(remaining) => remaining,
                    None => return Ok(self.min_fee_rate),
                };
                current_block_bytes = candidate.bytes;
                current_block_cycles = candidate.cycles;
            }
        }
        Ok(self.min_fee_rate)
    }
}
