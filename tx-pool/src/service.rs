//! Public controller protocol and the unified tx-pool service boundary.

pub(crate) mod builder;
pub(crate) mod controller;
pub(crate) mod dispatch;
pub(crate) mod message;

pub use builder::TxPoolServiceBuilder;
pub use controller::TxPoolController;
pub(crate) use dispatch::process;
pub(crate) use message::{
    AsyncRequest, Message, NotifyTxBatch, RemoteTxSubmission, SyncRequest, TestAcceptTxResult,
};
pub(crate) use message::{
    BlockTemplateResult, FeeEstimatesResult, FetchTxsWithCyclesResult,
    GetTransactionWithStatusResult, GetTxStatusResult, SubmitTxResult,
};

use ckb_channel::oneshot;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::BlockView,
    packed::{Byte32, ProposalShortId},
};
use std::{
    collections::{HashSet, VecDeque},
    fmt,
    sync::Arc,
};

/// Bounded controller queue. Backpressure begins here; the dispatcher never
/// creates more than its compiled number of owned handler tasks.
pub(crate) const DEFAULT_CHANNEL_SIZE: usize = 512;

/// Ordered reorg deltas are never dropped and retain at most one queued block
/// tree beyond the generation-owned consumer.
pub(crate) const REORG_CHANNEL_SIZE: usize = 1;

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

pub(crate) type ChainReorgArgs = (
    VecDeque<BlockView>,
    VecDeque<BlockView>,
    HashSet<ProposalShortId>,
    Arc<Snapshot>,
);

/// Committed verification outcome consumed by sync's known-transaction
/// projection.
#[derive(Clone, Debug)]
pub enum TxVerificationResult {
    Ok {
        original_peer: Option<PeerIndex>,
        tx_hash: Byte32,
    },
    UnknownParents {
        peer: PeerIndex,
        parents: HashSet<Byte32>,
    },
    Reject {
        tx_hash: Byte32,
    },
    GenerationReset,
}

/// Sole nonblocking receiver for the bounded committed relay projection.
pub struct TxVerificationResultReceiver(crate::authority::service::AuthorityRelayDrain);

impl TxVerificationResultReceiver {
    pub(crate) fn from_authority(receiver: crate::authority::service::AuthorityRelayDrain) -> Self {
        Self(receiver)
    }

    pub fn try_recv(&self) -> Option<TxVerificationResult> {
        self.0.try_recv()
    }

    pub fn drain(&self, limit: usize) -> Vec<TxVerificationResult> {
        std::iter::from_fn(|| self.try_recv()).take(limit).collect()
    }
}
