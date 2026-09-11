//! Public controller protocol and the unified tx-pool service boundary.

pub(crate) mod builder;
pub(crate) mod controller;
pub(crate) mod dispatch;
pub(crate) mod message;

pub use builder::TxPoolServiceBuilder;
pub use controller::TxPoolController;
pub(crate) use dispatch::process;
pub use message::RemoteTxBatchOutcome;
pub(crate) use message::{
    AdministrationGate, AdmittedAdministration, AsyncRequest, BoundedProposalIds,
    BoundedTransaction, BoundedTransactionError, BoundedTransactionHashes, ChainControl,
    ChainReorgArgs, ChainReorgPayloadLimit, Message, NotifyTxBatch, RemoteTxSubmission,
    RemoteTxSubmissionBatch, TestAcceptTxResult,
};
pub(crate) use message::{
    BlockTemplateResult, FeeEstimatesResult, FetchTxsWithCyclesResult,
    GetTransactionWithStatusResult, GetTxStatusResult, SubmitTxResult,
};

use ckb_channel::oneshot;
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use std::{collections::HashSet, fmt};

/// The requested owner existed at the public linearization point, but another
/// committed authority transition changed its exact removal cut before Apply.
/// Callers may retry only as a new explicit operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalRemovalCompetingProgress;

impl fmt::Display for LocalRemovalCompetingProgress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("tx-pool local removal lost to competing progress")
    }
}

impl std::error::Error for LocalRemovalCompetingProgress {}

/// Bounded controller queue. Backpressure begins here; the dispatcher never
/// creates more than its compiled number of owned handler tasks.
pub(crate) const DEFAULT_CHANNEL_SIZE: usize = 512;

/// Ordered chain/generation controls are never dropped and retain at most one
/// queued command beyond the generation-owned consumer.
pub(crate) const CHAIN_CONTROL_CHANNEL_SIZE: usize = 1;

pub(crate) trait OneshotSender<R: fmt::Debug> {
    fn send(self, value: R) -> Result<(), R>;
}

impl<R: fmt::Debug> OneshotSender<R> for oneshot::Sender<R> {
    fn send(self, value: R) -> Result<(), R> {
        oneshot::Sender::send(&self, value).map_err(|error| error.0)
    }
}

impl<R: fmt::Debug> OneshotSender<R> for tokio::sync::oneshot::Sender<R> {
    fn send(self, value: R) -> Result<(), R> {
        tokio::sync::oneshot::Sender::send(self, value)
    }
}

pub(crate) fn respond<R: fmt::Debug, S: OneshotSender<R>>(
    responder: S,
    value: R,
    message: &'static str,
) {
    if let Err(error) = responder.send(value) {
        ckb_logger::error!("Responder sending {message} failed {error:?}");
    }
}

pub(crate) struct Request<R, A> {
    pub responder: R,
    pub arguments: A,
}

impl<R, A> Request<R, A> {
    pub(crate) fn call(arguments: A, responder: R) -> Self {
        Self {
            responder,
            arguments,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Notify<A> {
    pub arguments: A,
}

impl<A> Notify<A> {
    pub(crate) fn new(arguments: A) -> Self {
        Self { arguments }
    }
}

/// Committed verification outcome consumed by sync's known-transaction
/// projection.
#[derive(Clone, Debug)]
pub enum TxVerificationResult {
    /// Verification completed and the transaction became known to tx-pool.
    Ok {
        /// Remote peer that originally supplied the transaction, when any.
        original_peer: Option<PeerIndex>,
        /// Canonical hash of the verified transaction.
        tx_hash: Byte32,
    },
    /// Verification cannot proceed until the listed parents are available.
    UnknownParents {
        /// Peer that supplied the transaction with missing parents.
        peer: PeerIndex,
        /// Canonical hashes of the unavailable parent transactions.
        parents: HashSet<Byte32>,
    },
    /// Verification rejected the transaction.
    Reject {
        /// Canonical hash of the rejected transaction.
        tx_hash: Byte32,
    },
    /// Discard stale known/pending relay state after committed detail was
    /// invalidated or coalesced.
    GenerationReset,
}

/// Sole nonblocking receiver for the bounded committed relay projection.
pub struct TxVerificationResultReceiver(crate::authority::service::RelayDrain);

impl TxVerificationResultReceiver {
    pub(crate) fn from_authority(receiver: crate::authority::service::RelayDrain) -> Self {
        Self(receiver)
    }

    /// Receives one committed result without waiting.
    pub fn try_recv(&self) -> Option<TxVerificationResult> {
        self.0.try_recv()
    }

    /// Waits until the bounded producer asks the sole consumer to drain.
    ///
    /// The signal carries no transaction data and coalesces while a drain is
    /// already pending. The periodic relayer tick remains the sparse-flow
    /// liveness fallback.
    pub async fn wait_for_drain(&self) {
        self.0.wait_for_drain().await;
    }

    /// Receives at most `limit` committed results without waiting.
    ///
    /// Allocation pressure returns the successfully reserved prefix and leaves
    /// every remaining result in the bounded authority-owned channel.
    pub fn drain(&self, limit: usize) -> Vec<TxVerificationResult> {
        self.0.drain(limit)
    }
}
