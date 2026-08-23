//! Public controller protocol and the unified tx-pool service boundary.

pub(crate) mod builder;
pub(crate) mod controller;
pub(crate) mod dispatch;
pub(crate) mod message;

pub(crate) use crate::authority::{BoundedTransaction, BoundedTransactionError};
pub use builder::TxPoolServiceBuilder;
pub use controller::TxPoolController;
pub(crate) use dispatch::process;
pub use message::RemoteTxBatchOutcome;
pub(crate) use message::{
    AsyncRequest, BoundedProposalIds, BoundedTransactionHashes, ChainControl, Message,
    NotifyTxBatch, RemoteTxSubmission, RemoteTxSubmissionBatch, SyncRequest, TestAcceptTxResult,
};
pub(crate) use message::{
    BlockTemplateResult, FeeEstimatesResult, FetchTxsWithCyclesResult,
    GetTransactionWithStatusResult, GetTxStatusResult, SubmitTxResult,
};

use ckb_app_config::TxPoolConfig;
use ckb_channel::oneshot;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{core::BlockView, packed::Byte32};
use std::{
    collections::{HashSet, VecDeque},
    fmt,
    sync::Arc,
};

/// Bounded controller queue. Backpressure begins here; the dispatcher never
/// creates more than its compiled number of owned handler tasks.
pub(crate) const DEFAULT_CHANNEL_SIZE: usize = 512;

/// Ordered chain/generation controls are never dropped and retain at most one
/// queued command beyond the generation-owned consumer.
pub(crate) const CHAIN_CONTROL_CHANNEL_SIZE: usize = 1;

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

/// Maximum detailed payload retained by one ordered reorg command.
///
/// The configured accepted and pre-pool residency budgets are the complete
/// transaction population that detailed reconciliation can preserve. A fork
/// payload larger than their checked sum cannot improve that preservation and
/// is represented by the constant-size safe generation replacement instead.
#[derive(Clone, Copy)]
pub(crate) struct ChainReorgPayloadLimit(usize);

impl ChainReorgPayloadLimit {
    pub(crate) fn from_config(config: &TxPoolConfig) -> Option<Self> {
        config
            .tx_pool_resident_size_budget()
            .checked_add(config.tx_pipeline_resident_size_budget())
            .map(Self)
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
    /// The authority generation changed before a transaction result committed.
    GenerationReset,
}

/// Sole nonblocking receiver for the bounded committed relay projection.
pub struct TxVerificationResultReceiver(crate::authority::service::AuthorityRelayDrain);

impl TxVerificationResultReceiver {
    pub(crate) fn from_authority(receiver: crate::authority::service::AuthorityRelayDrain) -> Self {
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
        let mut drained = Vec::new();
        while drained.len() < limit {
            // Relay observations are non-authoritative and nonblocking. Reserve
            // before consuming so allocation pressure returns the exact prefix
            // and leaves every unobserved result in the bounded channel.
            if drained.try_reserve(1).is_err() {
                break;
            }
            let Some(result) = self.try_recv() else {
                break;
            };
            drained.push(result);
        }
        drained
    }
}

#[cfg(test)]
#[path = "service/tests/support.rs"]
mod test_support;
