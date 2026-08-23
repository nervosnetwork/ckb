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
        tx_pool::{
            AncestorsScoreSortKey, PoolTxDetailInfo, TxEntryInfo, TxPoolEntryInfo, TxPoolIds,
        },
    },
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::Unpack,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Canonical Accepted relay observation returned by one authority read cut.
/// The transaction carries its own complete raw identity; a proposal short ID
/// is deliberately absent so a colliding request cannot alias another owner.
pub(crate) type AcceptedTransactionsWithCycles = Vec<(TransactionView, Cycle)>;

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

/// The sole admission gate and reusable storage for public full-pool reads.
///
/// It is derived scratch, never transaction authority. The runtime is a
/// cloneable cross-task handle, so every handle shares this one FIFO Tokio
/// mutex through the same `Arc`; contending async handlers suspend instead of
/// blocking a runtime thread. The contained vector grows only while no
/// authority guard is held and never exceeds the resource-ledger owner bound
/// requested at construction.
#[derive(Clone)]
pub(super) struct AuthorityQueryScratch {
    state: Arc<Mutex<AuthorityQueryScratchState>>,
}

struct AuthorityQueryScratchState {
    rows: Vec<FullQueryRow>,
    max_rows: usize,
}

/// Exclusive full-query admission plus the reusable prepared row storage.
/// Dropping the permit clears captured handles but retains capacity.
#[must_use = "a full-query permit must finish or discard its captured rows"]
pub(super) struct FullQueryPermit {
    state: OwnedMutexGuard<AuthorityQueryScratchState>,
}

enum FullQueryRow {
    PoolId {
        hash: RawTxHash,
        status: AcceptedStatus,
    },
    EntryInfo {
        hash: RawTxHash,
        status: AcceptedStatus,
        info: TxEntryInfo,
    },
    ReplacementHistory(RawTxHash),
    Fee(FeeCandidate),
}

#[must_use = "captured pool IDs must be materialized outside the authority guard"]
pub(super) struct PreparedPoolIds<'permit> {
    permit: &'permit mut FullQueryPermit,
    pending_count: usize,
    proposed_count: usize,
}

#[must_use = "captured pool entries must be materialized outside the authority guard"]
pub(super) struct PreparedEntryInfo<'permit> {
    permit: &'permit mut FullQueryPermit,
    pending_count: usize,
    proposed_count: usize,
    history_count: usize,
}

struct CapturedPoolDetail {
    timestamp: u64,
    status: AcceptedStatus,
    rank_in_pending: usize,
    pending_count: usize,
    proposed_count: usize,
    descendants_count: usize,
    ancestors_count: usize,
    score_sortkey: AncestorsScoreSortKey,
}

#[must_use = "captured pool detail must be materialized outside the authority guard"]
pub(super) struct PreparedPoolDetail<'permit> {
    _permit: &'permit mut FullQueryPermit,
    detail: Option<CapturedPoolDetail>,
}

#[must_use = "captured fee candidates must be materialized outside the authority guard"]
pub(super) struct PreparedFeeEstimate<'permit> {
    permit: &'permit mut FullQueryPermit,
    closest: BlockNumber,
    max_block_bytes: usize,
    max_block_cycles: Cycle,
    min_fee_rate: FeeRate,
    candidate_count: usize,
}

impl AuthorityQueryScratch {
    pub(super) fn new(max_rows: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(AuthorityQueryScratchState {
                rows: Vec::new(),
                max_rows,
            })),
        }
    }

    pub(super) async fn acquire(&self) -> FullQueryPermit {
        FullQueryPermit {
            state: Arc::clone(&self.state).lock_owned().await,
        }
    }
}

impl FullQueryPermit {
    /// Check the observed cut without allocating. `false` authorizes exactly
    /// one later lock-external growth step; exceeding the configured owner
    /// ceiling is a structural projection contradiction.
    pub(super) fn is_prepared(&self, observed_rows: usize) -> Result<bool, AuthorityQueryError> {
        if observed_rows > self.state.max_rows {
            return Err(AuthorityQueryError::Projection);
        }
        Ok(self.state.rows.capacity() >= observed_rows)
    }

    /// Grow geometrically outside the authority guard. Every successful call
    /// raises prepared capacity and therefore strictly decreases the finite
    /// `max_rows - prepared_rows` rank.
    pub(super) fn grow(&mut self, observed_rows: usize) -> Result<(), AuthorityQueryError> {
        if observed_rows > self.state.max_rows {
            return Err(AuthorityQueryError::Projection);
        }
        let capacity = self.state.rows.capacity().min(self.state.max_rows);
        if capacity >= observed_rows {
            return Ok(());
        }
        let doubled = capacity
            .checked_mul(2)
            .map_or(self.state.max_rows, |value| value.min(self.state.max_rows));
        let target = doubled.max(1).max(observed_rows).min(self.state.max_rows);
        // `Vec::try_reserve_exact` interprets `additional` relative to length,
        // not capacity. Scratch rows are empty at every growth cut, so asking
        // only for `target - capacity` could make a second growth a no-op and
        // violate the finite-rank progress proof.
        let additional = target.saturating_sub(self.state.rows.len());
        self.state
            .rows
            .try_reserve_exact(additional)
            .map_err(|_| AuthorityQueryError::Allocation)?;
        if self.state.rows.capacity() <= capacity || self.state.rows.capacity() < observed_rows {
            return Err(AuthorityQueryError::Projection);
        }
        Ok(())
    }

    pub(super) fn capture_pool_summary(
        &mut self,
        view: &AuthorityReadView<'_>,
        snapshot: &Snapshot,
    ) -> Result<AuthorityPoolSummary, AuthorityReadError> {
        if !self.state.rows.is_empty() {
            return Err(AuthorityReadError::Projection);
        }
        AuthorityPoolSummary::capture(snapshot, view.summary()?)
    }

    pub(super) fn capture_pool_ids<'permit>(
        &'permit mut self,
        view: &AuthorityReadView<'_>,
    ) -> Result<PreparedPoolIds<'permit>, AuthorityReadError> {
        self.begin_capture(view.owner_count())?;
        let (pending_count, proposed_count) = view.accepted_status_counts()?;
        for order in view.accepted_order() {
            let accepted = view.accepted_entry_for_order(order)?;
            self.push(FullQueryRow::PoolId {
                hash: order.hash().clone(),
                status: accepted.entry().status(),
            })?;
        }
        let captured = self.state.rows.len();
        let expected = pending_count
            .checked_add(proposed_count)
            .ok_or(AuthorityReadError::Arithmetic)?;
        if captured != expected {
            return Err(AuthorityReadError::Projection);
        }
        Ok(PreparedPoolIds {
            permit: self,
            pending_count,
            proposed_count,
        })
    }

    pub(super) fn capture_entry_info<'permit>(
        &'permit mut self,
        view: &AuthorityReadView<'_>,
    ) -> Result<PreparedEntryInfo<'permit>, AuthorityReadError> {
        self.begin_capture(view.owner_count())?;
        let (pending_count, proposed_count) = view.accepted_status_counts()?;
        for order in view.accepted_order().rev() {
            let accepted = view.accepted_entry_for_order(order)?;
            self.push(FullQueryRow::EntryInfo {
                hash: order.hash().clone(),
                status: accepted.entry().status(),
                info: entry_info(&accepted)?,
            })?;
        }
        let accepted_count = self.state.rows.len();
        let expected = pending_count
            .checked_add(proposed_count)
            .ok_or(AuthorityReadError::Arithmetic)?;
        if accepted_count != expected {
            return Err(AuthorityReadError::Projection);
        }
        for hash in view.replacement_history()? {
            self.push(FullQueryRow::ReplacementHistory(hash))?;
        }
        let history_count = self
            .state
            .rows
            .len()
            .checked_sub(accepted_count)
            .ok_or(AuthorityReadError::Arithmetic)?;
        Ok(PreparedEntryInfo {
            permit: self,
            pending_count,
            proposed_count,
            history_count,
        })
    }

    pub(super) fn capture_pool_detail<'permit>(
        &'permit mut self,
        view: &AuthorityReadView<'_>,
        hash: &RawTxHash,
    ) -> Result<PreparedPoolDetail<'permit>, AuthorityReadError> {
        if !self.state.rows.is_empty() {
            return Err(AuthorityReadError::Projection);
        }
        let Some(target) = view.accepted_entry_by_raw(hash)? else {
            return Ok(PreparedPoolDetail {
                _permit: self,
                detail: None,
            });
        };
        let (pending_count, proposed_count) = view.accepted_status_counts()?;
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
        let detail = CapturedPoolDetail {
            timestamp: target.entry().accepted_at.0,
            status: target.entry().status(),
            rank_in_pending,
            pending_count,
            proposed_count,
            descendants_count,
            ancestors_count,
            score_sortkey: target.order().score().clone().into(),
        };
        Ok(PreparedPoolDetail {
            _permit: self,
            detail: Some(detail),
        })
    }

    pub(super) fn capture_fee_estimate<'permit>(
        &'permit mut self,
        view: &AuthorityReadView<'_>,
        snapshot: &Snapshot,
        min_fee_rate: FeeRate,
    ) -> Result<PreparedFeeEstimate<'permit>, AuthorityReadError> {
        self.begin_capture(view.owner_count())?;
        let (pending_count, proposed_count) = view.accepted_status_counts()?;
        let candidate_count = pending_count
            .checked_add(proposed_count)
            .ok_or(AuthorityReadError::Arithmetic)?;
        for order in view.accepted_order().rev() {
            let accepted = view.accepted_entry_for_order(order)?;
            let metrics = accepted.entry().proof.metrics();
            self.push(FullQueryRow::Fee(FeeCandidate {
                fee: metrics.fee,
                bytes: metrics.cost.serialized_bytes,
                cycles: metrics.cost.cycles,
            }))?;
        }
        if self.state.rows.len() != candidate_count {
            return Err(AuthorityReadError::Projection);
        }
        Ok(PreparedFeeEstimate {
            permit: self,
            closest: snapshot.consensus().tx_proposal_window().closest(),
            max_block_bytes: usize::try_from(snapshot.consensus().max_block_bytes())
                .map_err(|_| AuthorityReadError::Arithmetic)?,
            max_block_cycles: snapshot.consensus().max_block_cycles(),
            min_fee_rate,
            candidate_count,
        })
    }

    fn begin_capture(&mut self, observed_rows: usize) -> Result<(), AuthorityReadError> {
        if !self.state.rows.is_empty()
            || observed_rows > self.state.max_rows
            || self.state.rows.capacity() < observed_rows
        {
            return Err(AuthorityReadError::Projection);
        }
        Ok(())
    }

    fn push(&mut self, row: FullQueryRow) -> Result<(), AuthorityReadError> {
        if self.state.rows.len() >= self.state.max_rows
            || self.state.rows.len() >= self.state.rows.capacity()
        {
            return Err(AuthorityReadError::Projection);
        }
        self.state.rows.push(row);
        Ok(())
    }
}

impl Drop for FullQueryPermit {
    fn drop(&mut self) {
        self.state.rows.clear();
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

impl PreparedPoolIds<'_> {
    pub(super) fn finish(self) -> Result<TxPoolIds, AuthorityReadError> {
        let mut pending = Vec::new();
        let mut proposed = Vec::new();
        pending
            .try_reserve(self.pending_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        proposed
            .try_reserve(self.proposed_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        for row in self.permit.state.rows.drain(..) {
            let FullQueryRow::PoolId { hash, status } = row else {
                return Err(AuthorityReadError::Projection);
            };
            match status {
                AcceptedStatus::Pending | AcceptedStatus::Gap => pending.push(hash.0),
                AcceptedStatus::Proposed => proposed.push(hash.0),
            }
        }
        if pending.len() != self.pending_count || proposed.len() != self.proposed_count {
            return Err(AuthorityReadError::Projection);
        }
        pending.sort_unstable();
        proposed.sort_unstable();
        Ok(TxPoolIds { pending, proposed })
    }
}

impl PreparedEntryInfo<'_> {
    pub(super) fn finish(self) -> Result<TxPoolEntryInfo, AuthorityReadError> {
        let mut pending = HashMap::new();
        let mut proposed = HashMap::new();
        pending
            .try_reserve(self.pending_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        proposed
            .try_reserve(self.proposed_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        let mut conflicted = Vec::new();
        conflicted
            .try_reserve(self.history_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        for row in self.permit.state.rows.drain(..) {
            match row {
                FullQueryRow::EntryInfo { hash, status, info } => {
                    let replaced = match status {
                        AcceptedStatus::Pending | AcceptedStatus::Gap => {
                            pending.insert(hash.0, info)
                        }
                        AcceptedStatus::Proposed => proposed.insert(hash.0, info),
                    };
                    if replaced.is_some() {
                        return Err(AuthorityReadError::Projection);
                    }
                }
                FullQueryRow::ReplacementHistory(hash) => conflicted.push(hash.0),
                FullQueryRow::PoolId { .. } | FullQueryRow::Fee(_) => {
                    return Err(AuthorityReadError::Projection);
                }
            }
        }
        if pending.len() != self.pending_count
            || proposed.len() != self.proposed_count
            || conflicted.len() != self.history_count
        {
            return Err(AuthorityReadError::Projection);
        }
        conflicted.sort_unstable();
        Ok(TxPoolEntryInfo {
            pending,
            proposed,
            conflicted,
        })
    }
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

impl PreparedPoolDetail<'_> {
    pub(super) fn finish(self) -> Result<Option<PoolTxDetailInfo>, AuthorityReadError> {
        let Some(detail) = self.detail else {
            return Ok(None);
        };
        let status = match detail.status {
            AcceptedStatus::Pending => "pending",
            AcceptedStatus::Gap => "gap",
            AcceptedStatus::Proposed => "proposed",
        };
        let mut entry_status = String::new();
        entry_status
            .try_reserve_exact(status.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        entry_status.push_str(status);
        Ok(Some(PoolTxDetailInfo {
            timestamp: detail.timestamp,
            entry_status,
            rank_in_pending: detail.rank_in_pending,
            pending_count: detail.pending_count,
            proposed_count: detail.proposed_count,
            descendants_count: detail.descendants_count,
            ancestors_count: detail.ancestors_count,
            score_sortkey: detail.score_sortkey,
        }))
    }
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
        let captured = view.compact_transactions(requested)?;
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
    requested: &[Byte32],
) -> Result<AcceptedTransactionsWithCycles, AuthorityReadError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(requested.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    for hash in requested {
        let Some(entry) = view.entry_by_raw(&RawTxHash(hash.clone())) else {
            continue;
        };
        if !matches!(entry.state(), super::read::AuthorityReadState::Accepted(_)) {
            continue;
        }
        let cycles = entry.cycles().ok_or(AuthorityReadError::Projection)?;
        result.push((entry.transaction().as_ref().clone(), cycles));
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

impl PreparedFeeEstimate<'_> {
    pub(super) fn finish(self) -> Result<FeeEstimateReadReceipt, AuthorityReadError> {
        let mut candidates = Vec::new();
        candidates
            .try_reserve(self.candidate_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        for row in self.permit.state.rows.drain(..) {
            let FullQueryRow::Fee(candidate) = row else {
                return Err(AuthorityReadError::Projection);
            };
            candidates.push(candidate);
        }
        if candidates.len() != self.candidate_count {
            return Err(AuthorityReadError::Projection);
        }
        Ok(FeeEstimateReadReceipt {
            closest: self.closest,
            max_block_bytes: self.max_block_bytes,
            max_block_cycles: self.max_block_cycles,
            min_fee_rate: self.min_fee_rate,
            candidates,
        })
    }
}

impl FeeEstimateReadReceipt {
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
