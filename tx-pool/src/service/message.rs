//! Tx-pool service message definitions.

use crate::{
    block_assembler::BoundedCandidateUncle,
    constants::ResidencyLimits,
    service::{Notify, Request},
};
use ckb_app_config::TxPoolConfig;
use ckb_channel::oneshot;
use ckb_error::AnyError;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        BlockView, Cycle, EstimateMode, FeeRate, TransactionView, Version,
        cell::CellStatus,
        tx_pool::{
            EntryCompleted, PoolTxDetailInfo, Reject, TRANSACTION_SIZE_LIMIT,
            TransactionWithStatus, TxPoolEntryInfo, TxPoolIds, TxPoolInfo, TxStatus,
        },
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

#[cfg(feature = "internal")]
use crate::{PlugTarget, component::entry::TxEntry};

pub(crate) type BlockTemplateResult = Result<BlockTemplate, AnyError>;
pub(crate) type BlockTemplateArgs = (Option<u64>, Option<u64>, Option<Version>);

pub(crate) type SubmitTxResult = Result<(), Reject>;

pub(crate) type TestAcceptTxResult = Result<EntryCompleted, Reject>;
pub(crate) type RemoveLocalTxResult = Result<bool, AnyError>;

pub(crate) type GetTxStatusResult = Result<(TxStatus, Option<Cycle>), AnyError>;
pub(crate) type GetTransactionWithStatusResult = Result<TransactionWithStatus, AnyError>;
pub(crate) type FetchTxsWithCyclesResult = crate::authority::query::AcceptedTransactionsWithCycles;

pub(crate) type FeeEstimatesResult = Result<FeeRate, AnyError>;

#[derive(Debug)]
pub(crate) struct BoundedTransaction {
    transaction: Arc<TransactionView>,
}
#[derive(Debug)]
pub(crate) enum BoundedTransactionError {
    TooLarge { actual: u64, maximum: u64 },
    Allocation,
}
impl BoundedTransaction {
    pub(crate) fn try_new(transaction: TransactionView) -> Result<Self, BoundedTransactionError> {
        let actual = u64::try_from(transaction.data().serialized_size_in_block())
            .map_err(|_| BoundedTransactionError::Allocation)?;
        if actual > TRANSACTION_SIZE_LIMIT {
            return Err(BoundedTransactionError::TooLarge {
                actual,
                maximum: TRANSACTION_SIZE_LIMIT,
            });
        }
        let transaction = transaction
            .try_into_compact()
            .map_err(|_| BoundedTransactionError::Allocation)?;
        Ok(Self {
            transaction: Arc::new(transaction),
        })
    }
    pub(crate) fn into_transaction(self) -> Arc<TransactionView> {
        self.transaction
    }
}

/// Canonical finite proposal-ID carrier. Public APIs keep their existing
/// collection signatures, but arbitrary-capacity caller containers never
/// cross the bounded service channel.
#[derive(Debug)]
pub(crate) struct BoundedProposalIds(Vec<ProposalShortId>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundedIdentifierSequenceError {
    TooMany { actual: usize, maximum: usize },
    Arithmetic,
    Allocation,
}

impl std::fmt::Display for BoundedIdentifierSequenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooMany { actual, maximum } => write!(
                formatter,
                "identifier request has {actual} items; maximum is {maximum}"
            ),
            Self::Arithmetic => formatter.write_str("identifier request size is not representable"),
            Self::Allocation => formatter.write_str("identifier request allocation unavailable"),
        }
    }
}

impl std::error::Error for BoundedIdentifierSequenceError {}

impl BoundedProposalIds {
    pub(crate) fn try_from_vec(
        ids: Vec<ProposalShortId>,
    ) -> Result<Self, BoundedIdentifierSequenceError> {
        Self::try_from_vec_with_limit(ids, ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH)
    }

    pub(crate) fn try_from_set(
        ids: HashSet<ProposalShortId>,
    ) -> Result<Self, BoundedIdentifierSequenceError> {
        let mut bounded = Self::try_from_iter_with_limit(
            ids.into_iter(),
            ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH,
        )?;
        bounded.0.sort_unstable();
        Ok(bounded)
    }

    fn try_from_vec_with_limit(
        ids: Vec<ProposalShortId>,
        maximum: usize,
    ) -> Result<Self, BoundedIdentifierSequenceError> {
        Self::try_from_iter_with_limit(ids.into_iter(), maximum)
    }

    fn try_from_iter_with_limit(
        ids: impl ExactSizeIterator<Item = ProposalShortId>,
        maximum: usize,
    ) -> Result<Self, BoundedIdentifierSequenceError> {
        let actual = ids.len();
        if actual > maximum {
            return Err(BoundedIdentifierSequenceError::TooMany { actual, maximum });
        }
        let normalized =
            crate::util::try_compact_proposal_ids(ids).map_err(|error| match error {
                crate::util::FixedPackedSequenceError::Arithmetic => {
                    BoundedIdentifierSequenceError::Arithmetic
                }
                crate::util::FixedPackedSequenceError::Allocation => {
                    BoundedIdentifierSequenceError::Allocation
                }
            })?;
        Ok(Self(normalized))
    }

    pub(super) fn into_vec(self) -> Vec<ProposalShortId> {
        self.0
    }
}

/// Canonical finite full-transaction-hash carrier for relay lookup. The raw
/// 32-byte identity is preserved end to end; proposal short IDs belong only to
/// compact-block and proposal protocol paths and cannot cross this boundary.
#[derive(Debug)]
pub(crate) struct BoundedTransactionHashes(Vec<Byte32>);

impl BoundedTransactionHashes {
    pub(crate) fn try_from_set(
        hashes: HashSet<Byte32>,
    ) -> Result<Self, BoundedIdentifierSequenceError> {
        Self::try_from_iter_with_limit(
            hashes.into_iter(),
            ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH,
        )
    }

    fn try_from_iter_with_limit(
        hashes: impl ExactSizeIterator<Item = Byte32>,
        maximum: usize,
    ) -> Result<Self, BoundedIdentifierSequenceError> {
        let actual = hashes.len();
        if actual > maximum {
            return Err(BoundedIdentifierSequenceError::TooMany { actual, maximum });
        }
        let mut normalized =
            crate::util::try_compact_transaction_hashes(hashes).map_err(|error| match error {
                crate::util::FixedPackedSequenceError::Arithmetic => {
                    BoundedIdentifierSequenceError::Arithmetic
                }
                crate::util::FixedPackedSequenceError::Allocation => {
                    BoundedIdentifierSequenceError::Allocation
                }
            })?;
        normalized.sort_unstable();
        Ok(Self(normalized))
    }

    pub(super) fn into_vec(self) -> Vec<Byte32> {
        self.0
    }
}

/// Relay transaction batch proven safe to retain in the tx-pool dispatcher.
///
/// The public controller keeps accepting `Vec<TransactionView>` for API
/// compatibility, but only this validated type may cross the bounded channel.
/// Its limits are the same protocol constants used by the relayer, so the
/// upstream network proof cannot be lost at the tx-pool boundary.
#[derive(Debug)]
pub(crate) struct NotifyTxBatch {
    pub(super) transactions: Vec<BoundedTransaction>,
}

#[derive(Debug, PartialEq, Eq)]
enum NotifyTxBatchError {
    TooMany { actual: usize, maximum: usize },
    TooLarge { actual: usize, maximum: usize },
    TransactionTooLarge { actual: u64, maximum: u64 },
    SizeOverflow,
    Allocation,
}

impl std::fmt::Display for NotifyTxBatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooMany { actual, maximum } => {
                write!(
                    formatter,
                    "relay transaction batch has {actual} items; maximum is {maximum}"
                )
            }
            Self::TooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "relay transaction batch has {actual} bytes; maximum is {maximum}"
                )
            }
            Self::TransactionTooLarge { actual, maximum } => write!(
                formatter,
                "relay transaction has {actual} serialized bytes; maximum is {maximum}"
            ),
            Self::SizeOverflow => formatter.write_str("relay transaction batch size overflowed"),
            Self::Allocation => {
                formatter.write_str("relay transaction batch allocation unavailable")
            }
        }
    }
}

impl std::error::Error for NotifyTxBatchError {}

impl NotifyTxBatch {
    pub(crate) fn try_new(txs: Vec<TransactionView>) -> Result<Self, AnyError> {
        Self::try_new_with_limits(
            txs,
            ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH,
            ckb_constant::sync::MAX_RELAY_TXS_BYTES_PER_BATCH,
        )
        .map_err(Into::into)
    }

    fn try_new_with_limits(
        txs: Vec<TransactionView>,
        max_count: usize,
        max_bytes: usize,
    ) -> Result<Self, NotifyTxBatchError> {
        if txs.len() > max_count {
            return Err(NotifyTxBatchError::TooMany {
                actual: txs.len(),
                maximum: max_count,
            });
        }
        let bytes = txs.iter().try_fold(0usize, |total, tx| {
            total
                .checked_add(tx.data().total_size())
                .ok_or(NotifyTxBatchError::SizeOverflow)
        })?;
        if bytes > max_bytes {
            return Err(NotifyTxBatchError::TooLarge {
                actual: bytes,
                maximum: max_bytes,
            });
        }
        let mut transactions = Vec::new();
        transactions
            .try_reserve_exact(txs.len())
            .map_err(|_| NotifyTxBatchError::Allocation)?;
        for tx in txs {
            transactions.push(
                BoundedTransaction::try_new(tx).map_err(|error| match error {
                    BoundedTransactionError::TooLarge { actual, maximum } => {
                        NotifyTxBatchError::TransactionTooLarge { actual, maximum }
                    }
                    BoundedTransactionError::Allocation => NotifyTxBatchError::Allocation,
                })?,
            );
        }
        Ok(Self { transactions })
    }

    pub(super) fn into_transactions(self) -> Vec<BoundedTransaction> {
        self.transactions
    }
}

/// A remote controller submission whose origin cannot be confused with Local
/// or Proposal admission at the service boundary.
pub(crate) struct RemoteTxSubmission {
    pub(crate) transaction: BoundedTransaction,
    pub(crate) declared_cycles: Cycle,
    pub(crate) peer: PeerIndex,
}

/// One network relay message retained as one bounded controller capability.
///
/// The vector is bounded by the same count and byte limits as the wire
/// message. This is also the already-declared bound of `NotifyTxBatch`, so the
/// new variant does not enlarge the controller channel's per-slot envelope.
#[derive(Debug)]
pub(crate) struct RemoteTxSubmissionBatch {
    submissions: Vec<(BoundedTransaction, Cycle)>,
    peer: PeerIndex,
}

impl RemoteTxSubmissionBatch {
    pub(crate) fn try_new(
        submissions: Vec<(TransactionView, Cycle)>,
        peer: PeerIndex,
    ) -> Result<Self, AnyError> {
        let actual = submissions.len();
        if actual > ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH {
            return Err(NotifyTxBatchError::TooMany {
                actual,
                maximum: ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH,
            }
            .into());
        }
        let bytes = submissions.iter().try_fold(0usize, |total, (tx, _)| {
            total
                .checked_add(tx.data().total_size())
                .ok_or(NotifyTxBatchError::SizeOverflow)
        })?;
        if bytes > ckb_constant::sync::MAX_RELAY_TXS_BYTES_PER_BATCH {
            return Err(NotifyTxBatchError::TooLarge {
                actual: bytes,
                maximum: ckb_constant::sync::MAX_RELAY_TXS_BYTES_PER_BATCH,
            }
            .into());
        }
        let mut bounded = Vec::new();
        bounded
            .try_reserve_exact(actual)
            .map_err(|_| NotifyTxBatchError::Allocation)?;
        for (tx, declared_cycles) in submissions {
            bounded.push((
                BoundedTransaction::try_new(tx).map_err(|error| match error {
                    BoundedTransactionError::TooLarge { actual, maximum } => {
                        NotifyTxBatchError::TransactionTooLarge { actual, maximum }
                    }
                    BoundedTransactionError::Allocation => NotifyTxBatchError::Allocation,
                })?,
                declared_cycles,
            ));
        }
        Ok(Self {
            submissions: bounded,
            peer,
        })
    }

    pub(super) fn into_parts(self) -> (PeerIndex, Vec<(BoundedTransaction, Cycle)>) {
        (self.peer, self.submissions)
    }
}

/// Total result of handing one bounded network batch to tx-pool authority.
///
/// `completed` counts the canonical input prefix whose ingress handling finished,
/// including published rejection notices. It does not count verified or accepted
/// transactions. Release relayer known marks for the remaining suffix without
/// synthesizing transaction rejections; an interrupted/faulted operation does not
/// provide that suffix with a successful outcome.
#[derive(Debug)]
pub struct RemoteTxBatchOutcome {
    offered: usize,
    completed: usize,
    error: Option<AnyError>,
}

impl RemoteTxBatchOutcome {
    pub(crate) fn complete(offered: usize) -> Self {
        Self {
            offered,
            completed: offered,
            error: None,
        }
    }

    pub(crate) fn failed(offered: usize, completed: usize, error: AnyError) -> Self {
        Self {
            offered,
            completed,
            error: Some(error),
        }
    }

    /// Number of transactions in the bounded network batch.
    pub fn offered(&self) -> usize {
        self.offered
    }

    /// Number of inputs with completed ingress handling, including rejections.
    pub fn completed(&self) -> usize {
        self.completed
    }

    /// Consume the outcome into offered count, completed-input count and the
    /// optional operation failure for the remaining suffix.
    pub fn into_parts(self) -> (usize, usize, Option<AnyError>) {
        (self.offered, self.completed, self.error)
    }
}

impl RemoteTxSubmission {
    pub(crate) fn new(
        transaction: BoundedTransaction,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Self {
        Self {
            transaction,
            declared_cycles,
            peer,
        }
    }
}

pub(crate) enum Message {
    BlockTemplate(SyncRequest<BlockTemplateArgs, BlockTemplateResult>),
    SubmitLocalTx(SyncRequest<BoundedTransaction, SubmitTxResult>),
    RemoveLocalTx(SyncRequest<Byte32, RemoveLocalTxResult>),
    TestAcceptTx(SyncRequest<BoundedTransaction, TestAcceptTxResult>),
    SubmitRemoteTx(AsyncRequest<RemoteTxSubmission, ()>),
    SubmitRemoteTxBatch(AsyncRequest<RemoteTxSubmissionBatch, RemoteTxBatchOutcome>),
    NotifyTxs(Notify<NotifyTxBatch>),
    FreshProposalsFilter(AsyncRequest<BoundedProposalIds, Vec<ProposalShortId>>),
    FetchTxs(AsyncRequest<BoundedProposalIds, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(AsyncRequest<BoundedTransactionHashes, FetchTxsWithCyclesResult>),
    GetTxPoolInfo(SyncRequest<(), TxPoolInfo>),
    GetLiveCell(SyncRequest<(OutPoint, bool), CellStatus>),
    GetTxStatus(SyncRequest<Byte32, GetTxStatusResult>),
    GetTransactionWithStatus(SyncRequest<Byte32, GetTransactionWithStatusResult>),
    NewUncle(Notify<BoundedCandidateUncle>),
    GetAllEntryInfo(SyncRequest<(), TxPoolEntryInfo>),
    GetAllIds(SyncRequest<(), TxPoolIds>),
    SavePool(SyncRequest<(), ()>),
    GetPoolTxDetails(SyncRequest<Byte32, PoolTxDetailInfo>),
    GetTotalRecentRejectNum(SyncRequest<(), Option<u64>>),

    UpdateIBDState(SyncRequest<bool, ()>),
    EstimateFeeRate(SyncRequest<(EstimateMode, bool), FeeEstimatesResult>),

    // test
    #[cfg(feature = "internal")]
    PlugEntry(SyncRequest<(Vec<TxEntry>, PlugTarget), Result<(), Reject>>),
    #[cfg(feature = "internal")]
    PackageTxs(SyncRequest<Option<u64>, Vec<TxEntry>>),
    SubmitLocalTestTx(SyncRequest<BoundedTransaction, SubmitTxResult>),
}

impl Message {
    pub(super) fn uses_reserved_read_handlers(&self, callback: bool) -> bool {
        // VM work and template-refresh waits use ordinary bounded handlers.
        // A publisher callback retains the reserved route for these reads:
        // ordinary handlers can be waiting for that same callback to return.
        match self {
            Self::TestAcceptTx(_) | Self::BlockTemplate(_) => callback,
            #[cfg(feature = "internal")]
            Self::PackageTxs(_) => callback,
            Self::FreshProposalsFilter(_)
            | Self::FetchTxs(_)
            | Self::FetchTxsWithCycles(_)
            | Self::GetTxPoolInfo(_)
            | Self::GetLiveCell(_)
            | Self::GetTxStatus(_)
            | Self::GetTransactionWithStatus(_)
            | Self::GetAllEntryInfo(_)
            | Self::GetAllIds(_)
            | Self::GetPoolTxDetails(_)
            | Self::GetTotalRecentRejectNum(_)
            | Self::EstimateFeeRate(_) => true,
            Self::SubmitLocalTx(_)
            | Self::RemoveLocalTx(_)
            | Self::SubmitRemoteTx(_)
            | Self::SubmitRemoteTxBatch(_)
            | Self::NotifyTxs(_)
            | Self::NewUncle(_)
            | Self::SavePool(_)
            | Self::UpdateIBDState(_)
            | Self::SubmitLocalTestTx(_) => false,
            #[cfg(feature = "internal")]
            Self::PlugEntry(_) => false,
        }
    }
}

mod administration {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    /// Shared public-administration gate across every cloneable controller.
    #[derive(Clone)]
    pub(crate) struct AdministrationGate {
        occupied: Arc<AtomicBool>,
    }

    impl AdministrationGate {
        pub(crate) fn new() -> Self {
            Self {
                occupied: Arc::new(AtomicBool::new(false)),
            }
        }

        pub(in crate::service) fn try_acquire(&self) -> Option<AdminAdmission> {
            self.occupied
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .ok()
                .map(|_| AdminAdmission { gate: self.clone() })
        }
    }

    /// Unique public-administration admission across every cloneable controller.
    ///
    /// The capability is moved into the ordered command and held through its
    /// execution. Its sealed representation prevents a clear from entering the
    /// reliable lane without first reserving the sole public payload slot.
    #[must_use = "an admitted administration must be sent or released"]
    pub(crate) struct AdminAdmission {
        gate: AdministrationGate,
    }

    impl Drop for AdminAdmission {
        fn drop(&mut self) {
            self.gate.occupied.store(false, Ordering::Release);
        }
    }

    /// An ordered public command that owns the sole admission capability.
    #[must_use = "an admitted administration must be consumed by the ordered driver"]
    pub(crate) struct AdmittedAdministration<R> {
        admission: AdminAdmission,
        request: R,
    }

    impl<R> AdmittedAdministration<R> {
        pub(in crate::service) fn new(admission: AdminAdmission, request: R) -> Self {
            Self { admission, request }
        }

        pub(crate) fn into_parts(self) -> (AdminAdmission, R) {
            (self.admission, self.request)
        }
    }
}

pub(crate) use administration::{AdministrationGate, AdmittedAdministration};

/// Maximum detailed payload retained by one ordered reorg command.
///
/// The derived accepted and pre-pool residency budgets are the complete
/// transaction population that detailed reconciliation can preserve. A fork
/// payload larger than their checked sum cannot improve that preservation and
/// is represented by the constant-size safe generation replacement instead.
#[derive(Clone, Copy)]
pub(crate) struct ChainReorgPayloadLimit(usize);

impl ChainReorgPayloadLimit {
    pub(crate) fn from_config(config: &TxPoolConfig) -> Option<Self> {
        let residency = ResidencyLimits::from_pool_size(config.max_tx_pool_size)?;
        residency.accepted.checked_add(residency.pipeline).map(Self)
    }
}

/// Sealed ordered chain input. Only a payload which refines the configured
/// count/byte bound may retain detailed fork collections in the capacity-one
/// lane; every oversize, overflow or normalization-allocation outcome carries
/// only the exact snapshot needed for a safe empty-generation replacement.
pub(crate) enum ChainReorgArgs {
    Detailed {
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        snapshot: Arc<Snapshot>,
    },
    ReplaceGeneration {
        snapshot: Arc<Snapshot>,
    },
}

impl ChainReorgArgs {
    pub(crate) fn bounded(
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        snapshot: Arc<Snapshot>,
        limit: ChainReorgPayloadLimit,
    ) -> Self {
        let charge = detached_blocks
            .iter()
            .chain(attached_blocks.iter())
            .try_fold(std::mem::size_of::<Arc<Snapshot>>(), |total, block| {
                total
                    .checked_add(std::mem::size_of::<BlockView>())?
                    .checked_add(block.data().total_size())
            });
        if charge.is_none_or(|charge| charge > limit.0) {
            return Self::ReplaceGeneration { snapshot };
        }

        let mut normalized_detached = VecDeque::new();
        let mut normalized_attached = VecDeque::new();
        if normalized_detached
            .try_reserve_exact(detached_blocks.len())
            .is_err()
            || normalized_attached
                .try_reserve_exact(attached_blocks.len())
                .is_err()
        {
            return Self::ReplaceGeneration { snapshot };
        }
        normalized_detached.extend(detached_blocks);
        normalized_attached.extend(attached_blocks);
        Self::Detailed {
            detached_blocks: normalized_detached,
            attached_blocks: normalized_attached,
            snapshot,
        }
    }
}

/// Rare generation controls that must preserve producer order with chain
/// reconciliation.
///
/// Keeping these commands on one bounded lane prevents a clear that follows
/// an installed chain transition from being overtaken by that transition's
/// detached-transaction recovery. Ordinary admission and read traffic remain
/// on the concurrent dispatcher.
pub(crate) enum ChainControl {
    Reconcile(SyncRequest<ChainReorgArgs, ()>),
    /// Replace the tx-pool snapshot, clear **all** accepted entries, and retire
    /// every pre-pool location as one generation.
    ClearPool(AdmittedAdministration<SyncRequest<Arc<Snapshot>, ()>>),
    /// Retire every pre-pool location as one generation without touching the
    /// accepted pool.
    ClearPipeline(AdmittedAdministration<SyncRequest<(), ()>>),
}

/// Synchronous request using the `ckb_channel` oneshot responder.
pub(crate) type SyncRequest<A, T> = Request<oneshot::Sender<T>, A>;
/// Asynchronous request using the `tokio` oneshot responder.
pub(crate) type AsyncRequest<A, T> = Request<tokio::sync::oneshot::Sender<T>, A>;

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;

#[cfg(test)]
#[path = "tests/message.rs"]
mod tests;
