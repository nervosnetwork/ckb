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
        let Ok(epoch) = self.current_pipeline_epoch() else {
            self.terminal_internal(tx, source).await;
            return;
        };
        self.after_process_at(tx, source, ret, epoch).await;
    }

    pub(crate) async fn after_process_at(
        &self,
        tx: TransactionView,
        source: TxSource,
        ret: &Result<Completed, Reject>,
        epoch: u64,
    ) {
        if !self.is_pipeline_epoch_current(epoch) {
            self.terminal_internal(tx, source).await;
            return;
        }
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

        // A held candidate (parked as `RaceLost`) surfaced here as
        // `RBFRejected` — e.g. the RPC/direct path flattening
        // `Superseded` — is not terminally rejected: its fate follows the
        // winner's, so nothing terminal may happen to it (no conflict
        // park, no relay, no recent_reject record). Losers woken by a
        // committed winner (`finalize_rbf_candidate`) are no longer held
        // by the time they get here and proceed to the real terminal
        // handling below.
        if matches!(ret, Err(Reject::RBFRejected(_))) {
            let held = {
                let room = self.pipeline.waiting_room.read().await;
                room.contains_key(&tx.proposal_short_id())
            };
            if held {
                return;
            }
        }

        if matches!(
            ret,
            Err(Reject::RBFRejected(..) | Reject::Resolve(OutPointError::Dead(_)))
        ) {
            // A tx already held in the pipeline (e.g. a superseded candidate
            // parked as `RaceLost`) must not be double-parked into the
            // pool-side waiting room: its fate is decided by the winner's
            // lifecycle, and a second parked copy would race the
            // hold-and-restore machinery.
            let already_held = {
                let room = self.pipeline.waiting_room.read().await;
                room.contains_key(&tx.proposal_short_id())
            };
            if !already_held {
                let mut tx_pool = self.pool.tx_pool.write().await;
                if self.is_pipeline_epoch_current(epoch)
                    && tx_pool.pool_map.find_conflict_outpoint(&tx).is_some()
                {
                    tx_pool.record_conflict(tx.clone(), source);
                }
            }
        }

        // Proposal txs (compact-block proposals arriving out of order) are
        // routed to the orphan pool just like remote ones: their parents
        // are usually in-flight relay traffic. Local (RPC) submissions with
        // missing inputs are *not* parked — they are recorded as rejected
        // (upstream RPC semantics: the caller resubmits when ready).
        if matches!(source, TxSource::Proposal)
            && let Err(reject) = ret
            && is_missing_input(reject)
        {
            let parents = tx.unique_parents();
            self.handle_missing_input_orphan_at(tx, source, parents, epoch)
                .await;
            return;
        }

        match source {
            TxSource::Remote { cycles, peer } => {
                self.after_process_remote(tx, peer, cycles, ret, epoch)
                    .await;
            }
            TxSource::Proposal => {
                // Proposal txs are a distinct source variant. For relay
                // purposes they are handled like local submissions, while the
                // verify queue uses `is_proposal_tx` to grant them priority.
                // They have no declared cycles or peer.
                self.after_process_local(tx, tx_hash, ret, epoch).await;
            }
            TxSource::Local => {
                self.after_process_local(tx, tx_hash, ret, epoch).await;
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

    /// Terminally reject a transaction whose bounded retries are exhausted.
    /// Unlike `after_process`, this bypasses the missing-input orphan
    /// parking — the whole point of the bounded retry is to give up.
    /// Recording and relaying behave like any other rejection.
    pub(crate) async fn terminal_reject(
        &self,
        tx: TransactionView,
        source: TxSource,
        reject: Reject,
    ) {
        match source {
            TxSource::Remote { peer, .. } => {
                self.handle_remote_reject(&tx.hash(), &reject, peer).await;
            }
            _ => {
                if reject.should_recorded() {
                    self.put_recent_reject(&tx.hash(), &reject);
                }
            }
        }
    }

    /// Terminal routing for a transaction whose processing failed
    /// *internally* — a panic caught by the per-job guard, or a bounded
    /// recovery that gave up. The transaction itself is not at fault, so
    /// nothing is recorded in recent_reject and no peer is banned; but the
    /// relayer must still hear a definitive answer, otherwise the peer's
    /// filter entry waits forever.
    pub(crate) async fn terminal_internal(&self, tx: TransactionView, source: TxSource) {
        debug!("terminal internal drop of {} from {:?}", tx.hash(), source);
        if source.peer().is_some() {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash: tx.hash() });
        }
    }

    /// True if the peer was banned within the ban window. Workers check
    /// popped jobs against this so a banned peer's in-flight jobs do not
    /// keep flowing into the pool: queue-level removal (`remove_by_peer`)
    /// only covers queued jobs, not ones a worker has already popped.
    pub(crate) fn is_recently_banned(&self, source: TxSource) -> bool {
        const DEFAULT_BAN_TIME: Duration = Duration::from_secs(MALFORMED_TX_BAN_SECONDS);
        let Some(peer) = source.peer() else {
            return false;
        };
        let banned = self.relay.banned_peers.lock().unwrap();
        banned
            .get(&peer)
            .is_some_and(|at| at.elapsed() < DEFAULT_BAN_TIME)
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
        epoch: u64,
    ) {
        let tx_hash = tx.hash();
        match ret {
            Ok(_) => {
                debug!(
                    "after_process remote send_result_to_relayer {} {}",
                    tx_hash, peer
                );
                self.handle_verify_success(&tx, Some(peer), epoch).await;
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
                    self.handle_missing_input_orphan_at(tx, source, parents, epoch)
                        .await;
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
        epoch: u64,
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
                self.handle_verify_success(&tx, None, epoch).await;
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
        epoch: u64,
    ) {
        if !self.is_pipeline_epoch_current(epoch) {
            self.terminal_internal(
                tx.clone(),
                original_peer.map_or(TxSource::Local, |peer| TxSource::Remote { cycles: 0, peer }),
            )
            .await;
            return;
        }
        self.send_result_to_relayer(TxVerificationResult::Ok {
            original_peer,
            tx_hash: tx.hash(),
        });
        Box::pin(self.process_orphan_tx_at(tx, epoch)).await;
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
        // A duplicate is not relayed as a rejection: the tx is (or will be)
        // in the pool, and a "Reject" would mark a valid transaction as
        // rejected in the relayer's peer filter. Local duplicates already
        // get the Ok re-broadcast treatment (see `after_process_local`).
        if reject.is_allowed_relay() && !matches!(reject, Reject::Duplicated(_)) {
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
        self.relay.network.ban_peer(peer, DEFAULT_BAN_TIME, reason);
        // Record the ban so workers can drop this peer's in-flight jobs
        // (popped from a queue before the ban) instead of letting them
        // into the pool; queue-level removal only covers queued jobs.
        {
            let mut banned = self.relay.banned_peers.lock().unwrap();
            let now = std::time::Instant::now();
            banned.retain(|_, at| now.saturating_duration_since(*at) < DEFAULT_BAN_TIME);
            banned.insert(peer, now);
        }
        self.pipeline
            .queues
            .ordered_resolve_queue
            .write()
            .await
            .remove_txs_by_peer(&peer);
        // Maintain the global lock order rbf_candidates -> verify_queue.
        // Acquire rbf_candidates first, then verify_queue, and clean up RBF
        // registrations for the removed txs while holding both locks. This
        // matches the order used in remove_tx and prevents deadlock with
        // register_rbf_candidate / update_tx_pool_for_reorg.
        let held = {
            let mut rbf = self.pipeline.queues.rbf_candidates.write().await;
            let removed_ids = self
                .pipeline
                .queues
                .verify_queue
                .write()
                .await
                .remove_txs_by_peer(&peer);
            let mut held = Vec::new();
            let mut room = self.pipeline.waiting_room.write().await;
            for id in &removed_ids {
                rbf.remove(id);
                held.extend(room.wake_by_winner(id));
            }
            held
        };
        // Restore candidates that were held by the banned peer's
        // registrations — unless they themselves came from the banned peer.
        self.restore_held_rbf_candidates(
            held.into_iter()
                .filter(|resolved| resolved.source.peer() != Some(peer))
                .collect(),
        )
        .await;
        // Remove orphan txs from the banned peer so they are not re-processed
        // after the ban.
        self.pipeline
            .waiting_room
            .write()
            .await
            .remove_by_peer(peer);
        self.pipeline.queues.pre_check_queue.remove_by_peer(&peer);
    }
}
