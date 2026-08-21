//! Tx-pool controller.

use crate::block_assembler::BoundedCandidateUncle;
use crate::error::{handle_recv_error, handle_send_cmd_error, handle_try_send_error};
use crate::service::{
    AdministrationGate, AdmittedAdministration, AsyncRequest, BlockTemplateResult,
    BoundedProposalIds, BoundedTransaction, BoundedTransactionError, BoundedTransactionHashes,
    ChainControl, ChainReorgArgs, ChainReorgPayloadLimit, FeeEstimatesResult,
    FetchTxsWithCyclesResult, GetTransactionWithStatusResult, GetTxStatusResult, Message, Notify,
    NotifyTxBatch, RemoteTxSubmission, Request, SubmitTxResult, TestAcceptTxResult,
};
use ckb_async_runtime::Handle;
use ckb_channel::oneshot;
use ckb_error::AnyError;
use ckb_logger::info;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        BlockView, Cycle, EstimateMode, TransactionView, UncleBlockView, Version,
        cell::CellStatus,
        tx_pool::{PoolTxDetailInfo, TxPoolEntryInfo, TxPoolIds, TxPoolInfo},
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc;
use tokio::task::block_in_place;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "internal")]
use crate::{PlugTarget, component::entry::TxEntry};

/// Controller to the tx-pool service.
///
/// The Controller is internally reference-counted and can be freely cloned. A Controller can be obtained when tx-pool service construct.
#[derive(Clone)]
pub struct TxPoolController {
    pub(crate) sender: mpsc::Sender<Message>,
    pub(crate) chain_control_sender: mpsc::Sender<ChainControl>,
    pub(crate) verification_command: crate::authority::service::AuthorityVerificationCommand,
    pub(crate) handle: Handle,
    pub(crate) started: Arc<AtomicBool>,
    pub(crate) administration_gate: AdministrationGate,
    pub(crate) chain_reorg_payload_limit: ChainReorgPayloadLimit,
    pub(crate) candidate_uncle_payload_limit: usize,
    pub(crate) signal: CancellationToken,
}

macro_rules! send_message {
    ($self:ident, $msg_type:ident, $args:expr) => {{
        let (responder, response) = oneshot::channel();
        let request = Request::call($args, responder);
        $self
            .sender
            .try_send(Message::$msg_type(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }};
}

macro_rules! send_notify {
    ($self:ident, $msg_type:ident, $args:expr) => {{
        let notify = Notify::new($args);
        $self
            .sender
            .try_send(Message::$msg_type(notify))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e.into()
            })
    }};
}

macro_rules! send_admitted_chain_control {
    ($self:ident, $command:ident, $args:expr) => {{
        let admission = $self
            .administration_gate
            .try_acquire()
            .ok_or_else(|| -> AnyError {
                ckb_error::OtherError::new(
                    "another tx-pool administration is already admitted".to_owned(),
                )
                .into()
            })?;
        let (responder, response) = oneshot::channel();
        let request = Request::call($args, responder);
        let command = ChainControl::$command(AdmittedAdministration::new(admission, request));
        block_in_place(|| {
            $self
                .handle
                .block_on($self.chain_control_sender.send(command))
        })
        .map_err(|error| {
            ckb_error::OtherError::new(format!("send ordered chain control fails: {error}"))
        })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }};
}

macro_rules! reject_callback_mutation {
    ($operation:literal) => {
        if crate::callback::in_callback() {
            return Err(ckb_error::OtherError::new(format!(
                "tx-pool callback cannot synchronously invoke mutating controller operation {}",
                $operation
            ))
            .into());
        }
    };
}

fn ingress_allocation_error() -> AnyError {
    ckb_error::OtherError::new("tx-pool transaction ingress allocation unavailable".to_owned())
        .into()
}

fn bounded_direct_transaction(
    transaction: TransactionView,
) -> Result<Result<BoundedTransaction, ckb_types::core::tx_pool::Reject>, AnyError> {
    match BoundedTransaction::try_new(transaction) {
        Ok(transaction) => Ok(Ok(transaction)),
        Err(BoundedTransactionError::TooLarge { actual, maximum }) => Ok(Err(
            ckb_types::core::tx_pool::Reject::ExceededTransactionSizeLimit(actual, maximum),
        )),
        Err(BoundedTransactionError::Allocation) => Err(ingress_allocation_error()),
    }
}

impl TxPoolController {
    /// Return whether tx-pool service is started
    pub fn service_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Return reference of tokio runtime handle
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Send a graceful stop signal to the tx-pool service and background workers.
    pub fn stop(&self) {
        self.signal.cancel();
    }

    /// Generate and return block_template
    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplateResult, AnyError> {
        send_message!(
            self,
            BlockTemplate,
            (bytes_limit, proposals_limit, max_version)
        )
    }

    /// Notify new uncle
    pub fn notify_new_uncle(&self, uncle: UncleBlockView) -> Result<(), AnyError> {
        let uncle = BoundedCandidateUncle::try_new(uncle, self.candidate_uncle_payload_limit)
            .map_err(|error| {
                ckb_error::OtherError::new(format!(
                    "tx-pool candidate-uncle ingress rejected: {error:?}"
                ))
            })?;
        send_notify!(self, NewUncle, uncle)
    }

    /// Make tx-pool consistent after a reorg, by re-adding or recursively erasing
    /// detached block transactions from the tx-pool, and also removing any
    /// other transactions from the tx-pool that are no longer valid given the new
    /// tip/height.
    pub fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) -> Result<(), AnyError> {
        reject_callback_mutation!("update_tx_pool_for_reorg");
        // Public compatibility facade only. Proposal position changes are
        // derived inside the authority from its paired old/new snapshots; a
        // caller-provided subset has no policy or cache-maintenance authority.
        drop(detached_proposal_id);
        let (responder, response) = oneshot::channel();
        let command = ChainControl::Reconcile(Request::call(
            ChainReorgArgs::bounded(
                detached_blocks,
                attached_blocks,
                snapshot,
                self.chain_reorg_payload_limit,
            ),
            responder,
        ));
        // Reorg messages are authoritative chain-state transitions, not
        // best-effort notifications. Dropping one when the bounded channel is
        // briefly full leaves committed transactions in the pool and loses
        // detached-transaction recovery permanently, because a later reorg
        // message is only a delta for that later fork. Apply backpressure to
        // the chain worker instead. `block_in_place` mirrors the controller's
        // synchronous request API while `handle.block_on` drives the async
        // bounded send without busy waiting.
        block_in_place(|| {
            self.handle
                .block_on(self.chain_control_sender.send(command))
        })
        .map_err(|error| {
            AnyError::from(ckb_error::OtherError::new(format!(
                "send chain reconciliation fails: {error}"
            )))
        })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Submit local tx to tx-pool
    pub fn submit_local_tx(&self, tx: TransactionView) -> Result<SubmitTxResult, AnyError> {
        reject_callback_mutation!("submit_local_tx");
        let tx = match bounded_direct_transaction(tx)? {
            Ok(tx) => tx,
            Err(reason) => return Ok(Err(reason)),
        };
        send_message!(self, SubmitLocalTx, tx)
    }

    /// test if a tx can be accepted by tx-pool
    /// Won't be broadcasted to network
    /// won't be insert to tx-pool
    pub fn test_accept_tx(&self, tx: TransactionView) -> Result<TestAcceptTxResult, AnyError> {
        let tx = match bounded_direct_transaction(tx)? {
            Ok(tx) => tx,
            Err(reason) => return Ok(Err(reason)),
        };
        send_message!(self, TestAcceptTx, tx)
    }

    /// Remove tx from tx-pool
    pub fn remove_local_tx(&self, tx_hash: Byte32) -> Result<bool, AnyError> {
        reject_callback_mutation!("remove_local_tx");
        send_message!(self, RemoveLocalTx, tx_hash)
    }

    /// Submit remote tx with declared cycles and origin to tx-pool
    pub async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<(), AnyError> {
        reject_callback_mutation!("submit_remote_tx");
        let (responder, response) = tokio::sync::oneshot::channel();
        let transaction = BoundedTransaction::try_new(tx).map_err(|error| match error {
            BoundedTransactionError::TooLarge { actual, maximum } => AnyError::from(
                ckb_types::core::tx_pool::Reject::ExceededTransactionSizeLimit(actual, maximum),
            ),
            BoundedTransactionError::Allocation => ingress_allocation_error(),
        })?;
        let request = AsyncRequest::call(
            RemoteTxSubmission::new(transaction, declared_cycles, peer),
            responder,
        );
        self.sender
            .try_send(Message::SubmitRemoteTx(request))
            .map_err(|error| {
                let (_, error) = handle_try_send_error(error);
                error
            })?;
        response.await.map_err(Into::into)
    }

    /// Receive txs from network, try to add txs to tx-pool
    pub fn notify_txs(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        send_notify!(self, NotifyTxs, NotifyTxBatch::try_new(txs)?)
    }

    /// Receive txs from network, try to add txs to tx-pool
    pub async fn notify_txs_async(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        let notify = Notify::new(NotifyTxBatch::try_new(txs)?);
        self.sender
            .try_send(Message::NotifyTxs(notify))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e.into()
            })
    }

    /// Return tx-pool information
    pub fn get_tx_pool_info(&self) -> Result<TxPoolInfo, AnyError> {
        send_message!(self, GetTxPoolInfo, ())
    }

    /// Return tx-pool information
    pub fn get_live_cell(
        &self,
        out_point: OutPoint,
        with_data: bool,
    ) -> Result<CellStatus, AnyError> {
        send_message!(self, GetLiveCell, (out_point, with_data))
    }

    /// Return fresh proposals
    pub async fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, AnyError> {
        let (responder, response) = tokio::sync::oneshot::channel();
        let request = AsyncRequest::call(BoundedProposalIds::try_from_vec(proposals)?, responder);
        self.sender
            .try_send(Message::FreshProposalsFilter(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        response.await.map_err(Into::into)
    }

    /// Return tx_status for rpc (get_transaction verbosity = 1)
    pub fn get_tx_status(&self, hash: Byte32) -> Result<GetTxStatusResult, AnyError> {
        send_message!(self, GetTxStatus, hash)
    }

    /// Return transaction_with_status for rpc (get_transaction verbosity = 2)
    pub fn get_transaction_with_status(
        &self,
        hash: Byte32,
    ) -> Result<GetTransactionWithStatusResult, AnyError> {
        send_message!(self, GetTransactionWithStatus, hash)
    }

    /// Mainly used for compact block reconstruction and block proposal pre-broadcasting
    /// Orphan/conflicted/etc transactions that are returned for compact block reconstruction.
    pub async fn fetch_txs(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, AnyError> {
        let (responder, response) = tokio::sync::oneshot::channel();
        let request = AsyncRequest::call(BoundedProposalIds::try_from_set(short_ids)?, responder);
        self.sender
            .try_send(Message::FetchTxs(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        response.await.map_err(Into::into)
    }

    /// Return accepted transactions with cycles by complete raw transaction
    /// hash. This identity is distinct from compact-block proposal IDs.
    pub async fn fetch_txs_with_cycles(
        &self,
        tx_hashes: HashSet<Byte32>,
    ) -> Result<FetchTxsWithCyclesResult, AnyError> {
        let (responder, response) = tokio::sync::oneshot::channel();
        let request = AsyncRequest::call(
            BoundedTransactionHashes::try_from_set(tx_hashes)?,
            responder,
        );
        self.sender
            .try_send(Message::FetchTxsWithCycles(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        response.await.map_err(Into::into)
    }

    /// Clears the tx-pool, removing all txs, update snapshot.
    pub fn clear_pool(&self, new_snapshot: Arc<Snapshot>) -> Result<(), AnyError> {
        reject_callback_mutation!("clear_pool");
        send_admitted_chain_control!(self, ClearPool, new_snapshot)
    }

    /// Clears every kernel-owned pre-pool lifecycle entry without
    /// touching the already-accepted pool. The method name is retained for
    /// controller API compatibility.
    pub fn clear_verify_queue(&self) -> Result<(), AnyError> {
        reject_callback_mutation!("clear_verify_queue");
        send_admitted_chain_control!(self, ClearPipeline, ())
    }

    /// Returns information about all transactions in the pool.
    pub fn get_all_entry_info(&self) -> Result<TxPoolEntryInfo, AnyError> {
        send_message!(self, GetAllEntryInfo, ())
    }

    /// Returns the IDs of all transactions in the pool.
    pub fn get_all_ids(&self) -> Result<TxPoolIds, AnyError> {
        send_message!(self, GetAllIds, ())
    }

    /// query the details of a transaction in the pool
    pub fn get_tx_detail(&self, tx_hash: Byte32) -> Result<PoolTxDetailInfo, AnyError> {
        send_message!(self, GetPoolTxDetails, tx_hash)
    }

    /// Saves tx pool into disk.
    pub fn save_pool(&self) -> Result<(), AnyError> {
        reject_callback_mutation!("save_pool");
        info!("Please be patient, tx-pool are saving data into disk ...");
        send_message!(self, SavePool, ())
    }

    /// Updates IBD state.
    pub fn update_ibd_state(&self, in_ibd: bool) -> Result<(), AnyError> {
        send_message!(self, UpdateIBDState, in_ibd)
    }

    /// Estimates fee rate.
    pub fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeEstimatesResult, AnyError> {
        send_message!(self, EstimateFeeRate, (estimate_mode, enable_fallback))
    }

    /// Sends suspend chunk process cmd
    pub fn suspend_chunk_process(&self) -> Result<(), AnyError> {
        //debug!("[verify-test] run suspend_chunk_process");
        self.verification_command
            .suspend()
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// Sends continue chunk process cmd
    pub fn continue_chunk_process(&self) -> Result<(), AnyError> {
        //debug!("[verify-test] run continue_chunk_process");
        self.verification_command
            .resume()
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// Plug tx-pool entry to tx-pool, skip verification. only for test
    #[cfg(feature = "internal")]
    pub fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) -> Result<(), AnyError> {
        reject_callback_mutation!("plug_entry");
        let response: Result<Result<(), crate::error::Reject>, AnyError> =
            send_message!(self, PlugEntry, (entries, target));
        response?.map_err(AnyError::from)
    }

    /// Package txs with specified bytes_limit. for test
    #[cfg(feature = "internal")]
    pub fn package_txs(&self, bytes_limit: Option<u64>) -> Result<Vec<TxEntry>, AnyError> {
        send_message!(self, PackageTxs, bytes_limit)
    }

    /// Submit a local transaction through the integration-test RPC and return
    /// its definitive validation/commit result synchronously.
    pub fn submit_local_test_tx(&self, tx: TransactionView) -> Result<SubmitTxResult, AnyError> {
        reject_callback_mutation!("submit_local_test_tx");
        let tx = match bounded_direct_transaction(tx)? {
            Ok(tx) => tx,
            Err(reason) => return Ok(Err(reason)),
        };
        send_message!(self, SubmitLocalTestTx, tx)
    }

    /// get total recent reject num
    pub fn get_total_recent_reject_num(&self) -> Result<Option<u64>, AnyError> {
        send_message!(self, GetTotalRecentRejectNum, ())
    }
}

#[cfg(test)]
#[path = "tests/controller.rs"]
mod tests;
