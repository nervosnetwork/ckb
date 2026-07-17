use crate::component::pipeline_queue::PipelineQueue;
use crate::constants::MALFORMED_TX_BAN_SECONDS;
use crate::error::Reject;
use crate::service::{TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use crate::util::is_missing_input;
use ckb_logger::{Level::Trace, debug, log_enabled_target, trace_target};
use ckb_network::PeerIndex;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{Cycle, TransactionView};
use ckb_types::packed::Byte32;
use ckb_verification::cache::Completed;
use std::time::Duration;

impl TxPoolService {
    pub(crate) async fn after_process(
        &self,
        tx: TransactionView,
        source: TxSource,
        ret: &Result<Completed, Reject>,
    ) {
        let tx_hash = tx.hash();

        // log tx verification result for monitor node
        if log_enabled_target!("ckb_tx_monitor", Trace)
            && let Ok(c) = ret
        {
            trace_target!(
                "ckb_tx_monitor",
                r#"{{"tx_hash":"{:#x}","cycles":{}}}"#,
                tx_hash,
                c.cycles
            );
        }

        if matches!(
            ret,
            Err(Reject::RBFRejected(..) | Reject::Resolve(OutPointError::Dead(_)))
        ) {
            let mut tx_pool = self.tx_pool.write().await;
            if tx_pool.pool_map.find_conflict_outpoint(&tx).is_some() {
                tx_pool.record_conflict(tx.clone(), source);
            }
        }

        match source {
            TxSource::Remote { cycles, peer } => {
                self.after_process_remote(tx, peer, cycles, ret).await;
            }
            TxSource::Proposal => {
                // Proposal txs are a distinct source variant. For relay
                // purposes they are handled like local submissions, while the
                // verify queue uses `is_proposal_tx` to grant them priority.
                // They have no declared cycles or peer.
                self.after_process_local(tx, tx_hash, ret).await;
            }
            TxSource::Local => {
                self.after_process_local(tx, tx_hash, ret).await;
            }
        }
    }
    /// Convenience helper: record a reject outcome for a transaction and run the
    /// shared after-process side effects (relayer notification, conflict cache
    /// update, local callbacks).
    pub(crate) async fn reject_with_after_process(
        &self,
        tx: TransactionView,
        source: TxSource,
        reject: Reject,
    ) {
        self.after_process(tx, source, &Err(reject)).await;
    }
    /// Post-process a remote transaction result.
    ///
    /// `peer` and `cycles` are passed explicitly rather than inside `TxSource`
    /// so the remote-only preconditions are visible in the signature. The
    /// `TxSource::Remote` variant is reconstructed only when the tx needs to be
    /// stored in the orphan pool.
    pub(crate) async fn after_process_remote(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        cycles: Cycle,
        ret: &Result<Completed, Reject>,
    ) {
        let tx_hash = tx.hash();
        match ret {
            Ok(_) => {
                debug!(
                    "after_process remote send_result_to_relayer {} {}",
                    tx_hash, peer
                );
                self.handle_verify_success(&tx, Some(peer)).await;
            }
            Err(reject) => {
                debug!(
                    "after_process {} {} remote reject: {} ",
                    tx_hash, peer, reject
                );
                if is_missing_input(reject) {
                    let parents = tx.unique_parents();
                    // Orphan storage still uses TxSource, so reconstruct the
                    // remote variant only for the missing-input path.
                    let source = TxSource::Remote { cycles, peer };
                    self.handle_missing_input_orphan(tx, source, parents).await;
                } else {
                    self.handle_remote_reject(&tx_hash, reject, peer).await;
                }
            }
        }
    }
    pub(crate) async fn after_process_local(
        &self,
        tx: TransactionView,
        tx_hash: Byte32,
        ret: &Result<Completed, Reject>,
    ) {
        match ret {
            Ok(_) | Err(Reject::Duplicated(_)) => {
                if matches!(ret, Err(Reject::Duplicated(_))) {
                    debug!("after_process {} duplicated", tx_hash);
                } else {
                    debug!("after_process local send_result_to_relayer {}", tx_hash);
                }
                // Re-broadcast tx when it's duplicated and submitted
                // through local rpc, or notify on fresh success.
                self.handle_verify_success(&tx, None).await;
            }
            Err(reject) => {
                debug!("after_process {} reject: {} ", tx_hash, reject);
                if reject.should_recorded() {
                    self.put_recent_reject(&tx_hash, reject);
                }
            }
        }
    }
    /// Common success handler: relay the result and trigger orphan processing.
    ///
    /// Box::pin is required because after_process and process_orphan_tx are
    /// mutually recursive async fns; without boxing the compiler cannot prove
    /// the resulting future has a finite size.
    pub(crate) async fn handle_verify_success(
        &self,
        tx: &TransactionView,
        original_peer: Option<PeerIndex>,
    ) {
        self.send_result_to_relayer(TxVerificationResult::Ok {
            original_peer,
            tx_hash: tx.hash(),
        });
        Box::pin(self.process_orphan_tx(tx)).await;
    }
    /// Post-processing for a rejected remote transaction: ban the peer if the
    /// tx is malformed, relay the rejection if allowed, and record it in the
    /// recent-reject database if applicable.
    ///
    /// This is the single source of truth for the "remote error triple" used
    /// by both [`Self::after_process`] and [`Self::process_orphan_tx`].
    pub(crate) async fn handle_remote_reject(
        &self,
        tx_hash: &Byte32,
        reject: &Reject,
        peer: PeerIndex,
    ) {
        if reject.is_malformed_tx() {
            self.ban_malformed(peer, format!("reject {reject}")).await;
        }
        if reject.is_allowed_relay() {
            self.send_result_to_relayer(TxVerificationResult::Reject {
                tx_hash: tx_hash.clone(),
            });
        }
        if reject.should_recorded() {
            self.put_recent_reject(tx_hash, reject);
        }
    }
    pub(crate) async fn ban_malformed(&self, peer: PeerIndex, reason: String) {
        const DEFAULT_BAN_TIME: Duration = Duration::from_secs(MALFORMED_TX_BAN_SECONDS);

        #[cfg(feature = "with_sentry")]
        use sentry::{Level, capture_message, with_scope};

        #[cfg(feature = "with_sentry")]
        with_scope(
            |scope| scope.set_fingerprint(Some(&["ckb-tx-pool", "receive-invalid-remote-tx"])),
            || {
                capture_message(
                    &format!(
                        "Ban peer {} for {} seconds, reason: \
                        {}",
                        peer,
                        DEFAULT_BAN_TIME.as_secs(),
                        reason
                    ),
                    Level::Info,
                )
            },
        );
        self.network.ban_peer(peer, DEFAULT_BAN_TIME, reason);
        self.queues
            .ordered_resolve_queue
            .write()
            .await
            .remove_txs_by_peer(&peer);
        // Maintain the global lock order rbf_candidates -> verify_queue.
        // Acquire rbf_candidates first, then verify_queue, and clean up RBF
        // registrations for the removed txs while holding both locks. This
        // matches the order used in remove_tx and prevents deadlock with
        // register_rbf_candidate / update_tx_pool_for_reorg.
        let _removed_ids = {
            let mut rbf = self.rbf_candidates.write().await;
            let removed_ids = self
                .queues
                .verify_queue
                .write()
                .await
                .remove_txs_by_peer(&peer);
            for id in &removed_ids {
                rbf.remove(id);
            }
            removed_ids
        };
        // Remove orphan txs from the banned peer so they are not re-processed
        // after the ban.
        self.orphan.write().await.remove_by_peer(peer);
        self.queues.pre_check_queue.remove_by_peer(&peer);
    }
}
