//! Tx-pool service message definitions.

use crate::service::{Notify, Request};
use crate::tx_source::TxSource;
use ckb_channel::oneshot;
use ckb_error::AnyError;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        Cycle, EstimateMode, FeeRate, TransactionView, UncleBlockView, Version,
        cell::CellStatus,
        tx_pool::{
            EntryCompleted, PoolTxDetailInfo, Reject, TransactionWithStatus, TxPoolEntryInfo,
            TxPoolIds, TxPoolInfo, TxStatus,
        },
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(feature = "internal")]
use crate::{component::entry::TxEntry, process::PlugTarget};

pub(crate) type BlockTemplateResult = Result<BlockTemplate, AnyError>;
pub(crate) type BlockTemplateArgs = (Option<u64>, Option<u64>, Option<Version>);

pub(crate) type SubmitTxResult = Result<(), Reject>;

pub(crate) type TestAcceptTxResult = Result<EntryCompleted, Reject>;

pub(crate) type GetTxStatusResult = Result<(TxStatus, Option<Cycle>), AnyError>;
pub(crate) type GetTransactionWithStatusResult = Result<TransactionWithStatus, AnyError>;
pub(crate) type FetchTxsWithCyclesResult = Vec<(ProposalShortId, (TransactionView, Cycle))>;

pub(crate) type FeeEstimatesResult = Result<FeeRate, AnyError>;

pub(crate) enum Message {
    BlockTemplate(SyncRequest<BlockTemplateArgs, BlockTemplateResult>),
    SubmitLocalTx(SyncRequest<TransactionView, SubmitTxResult>),
    RemoveLocalTx(SyncRequest<Byte32, bool>),
    TestAcceptTx(SyncRequest<TransactionView, TestAcceptTxResult>),
    SubmitRemoteTx(SyncRequest<(TransactionView, TxSource), ()>),
    NotifyTxs(Notify<Vec<TransactionView>>),
    FreshProposalsFilter(AsyncRequest<Vec<ProposalShortId>, Vec<ProposalShortId>>),
    FetchTxs(AsyncRequest<HashSet<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(AsyncRequest<HashSet<ProposalShortId>, FetchTxsWithCyclesResult>),
    GetTxPoolInfo(SyncRequest<(), TxPoolInfo>),
    GetLiveCell(SyncRequest<(OutPoint, bool), CellStatus>),
    GetTxStatus(SyncRequest<Byte32, GetTxStatusResult>),
    GetTransactionWithStatus(SyncRequest<Byte32, GetTransactionWithStatusResult>),
    NewUncle(Notify<UncleBlockView>),
    /// Replace the tx-pool snapshot, clear **all** in-pool entries, and drain
    /// all pipeline queues (ordered resolve, verification, orphan and pre-check).
    ClearPool(SyncRequest<Arc<Snapshot>, ()>),
    /// Clear only the pipeline queues (ordered resolve, verification, orphan
    /// and pre-check) without touching the already-accepted pool.
    ClearPipeline(SyncRequest<(), ()>),
    GetAllEntryInfo(SyncRequest<(), TxPoolEntryInfo>),
    GetAllIds(SyncRequest<(), TxPoolIds>),
    SavePool(SyncRequest<(), ()>),
    GetPoolTxDetails(SyncRequest<Byte32, PoolTxDetailInfo>),
    GetTotalRecentRejectNum(SyncRequest<(), Option<u64>>),

    UpdateIBDState(SyncRequest<bool, ()>),
    EstimateFeeRate(SyncRequest<(EstimateMode, bool), FeeEstimatesResult>),

    // test
    #[cfg(feature = "internal")]
    PlugEntry(SyncRequest<(Vec<TxEntry>, PlugTarget), ()>),
    #[cfg(feature = "internal")]
    PackageTxs(SyncRequest<Option<u64>, Vec<TxEntry>>),
    SubmitLocalTestTx(SyncRequest<TransactionView, SubmitTxResult>),
}

/// Synchronous request using the `ckb_channel` oneshot responder.
pub(crate) type SyncRequest<A, T> = Request<oneshot::Sender<T>, A>;
/// Asynchronous request using the `tokio` oneshot responder.
pub(crate) type AsyncRequest<A, T> = Request<tokio::sync::oneshot::Sender<T>, A>;

#[derive(Debug, Hash, Eq, PartialEq)]
pub(crate) enum BlockAssemblerMessage {
    Pending,
    Proposed,
    Uncle,
    Reset(Arc<Snapshot>),
}

/// Deferred side-effects that are processed by a single background worker
/// instead of fire-and-forget `tokio::spawn` calls.
pub(crate) enum DeferredTask {
    /// Push RBF-displaced transactions back into the ordered resolve queue
    /// so they can be re-resolved, re-verified and re-submitted.
    RecoverTxs(Vec<(TransactionView, TxSource)>),
    /// Store a successful verification result in the cache (keyed by wtx_hash).
    CacheUpdate {
        wtx_hash: Byte32,
        verified: ckb_verification::cache::Completed,
    },
}
