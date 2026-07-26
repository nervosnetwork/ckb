use crate::constants::MALFORMED_TX_BAN_SECONDS;
use crate::error::Reject;
use crate::service::effects::{
    EffectClass, EffectJournalError, TxPoolEffect, bounded_commit_ban_reason,
};
use crate::service::{TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use ckb_logger::{Level::Trace, debug, log_enabled_target, trace_target};
use ckb_network::PeerIndex;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{Cycle, TransactionView};
use ckb_types::packed::Byte32;
use ckb_verification::cache::Completed;
use std::time::Duration;

#[cfg(test)]
#[path = "tests/post_process_seam.rs"]
mod test_seam;

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

        if matches!(
            ret,
            Err(Reject::RBFRejected(..) | Reject::Resolve(OutPointError::Dead(_)))
        ) {
            let mut tx_pool = self.pool.tx_pool.write().await;
            let defect_snapshot = tx_pool.cloned_snapshot();
            let mutation = self.pipeline.kernel.guard_authoritative_mutation(
                "post-process conflict-history mutation panicked",
                || {
                    if self.is_pipeline_epoch_current(epoch)
                        && tx_pool.pool_map.find_conflict_outpoint(&tx).is_some()
                    {
                        let raw = crate::component::pre_pool::PipelineRawTx::new(
                            tx.clone(),
                            source,
                            epoch,
                        );
                        let keys = crate::component::pre_pool::conflict_dependency_keys(
                            &tx,
                            std::iter::empty(),
                        );
                        let owner = crate::component::pre_pool::historical_source(source);
                        let expires_at =
                            crate::component::pre_pool::historical_deadline(owner);
                        if let Err(error) = self.pipeline.kernel.mutate(|kernel| {
                            kernel.retain_conflict(raw, owner, keys, expires_at)
                        }) {
                            debug!(
                                "dropping bounded post-process conflict history for {tx_hash}: {error:?}"
                            );
                        }
                    }
                },
            );
            let disposal = if let Err(defect) = mutation {
                Some(self.recover_authoritative_defect_with_fingerprint(
                    &mut tx_pool,
                    defect_snapshot,
                    "post-process conflict-history mutation",
                    defect,
                    source.peer().is_some().then(|| tx.witness_hash()),
                ))
            } else {
                None
            };
            drop(tx_pool);
            drop(disposal);
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
    /// shared after-process side effects (relayer notification, historical
    /// conflict Wait update, local callbacks).
    pub(crate) async fn reject_with_after_process(
        &self,
        tx: TransactionView,
        source: TxSource,
        reject: Reject,
    ) {
        self.after_process(tx, source, &Err(reject)).await;
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
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash: tx.hash() })
                .await;
        }
    }

    /// True if the peer was banned within the ban window. Workers check
    /// popped jobs against this so a banned peer's in-flight jobs do not
    /// keep flowing into the pool: queue-level removal (`remove_by_peer`)
    /// only covers queued jobs, not ones a worker has already popped.
    pub(crate) fn is_recently_banned(&self, source: TxSource) -> bool {
        let Some(peer) = source.peer() else {
            return false;
        };
        self.relay.banned_peers.contains(peer)
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
        _cycles: Cycle,
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
                self.handle_remote_reject(&tx_hash, reject, peer).await;
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
                if let Some(effect) = self.recent_reject_effect(tx_hash, reject) {
                    self.publish_effects(vec![effect]).await;
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
        })
        .await;
    }
    /// Post-processing for a rejected remote transaction: ban the peer if the
    /// tx is malformed, relay the rejection if allowed, and record it in the
    /// recent-reject database if applicable.
    ///
    /// This is the single source of truth for remote terminal policy used by
    /// both [`Self::after_process`] and [`Self::process_orphan_tx`].
    pub(crate) async fn handle_remote_reject(
        &self,
        tx_hash: &Byte32,
        reject: &Reject,
        peer: PeerIndex,
    ) {
        // A duplicate is not relayed as a rejection: the tx is (or will be)
        // in the pool, and a "Reject" would mark a valid transaction as
        // rejected in the relayer's peer filter. Local duplicates already
        // get the Ok re-broadcast treatment (see `after_process_local`).
        let ban_reason = reject
            .is_malformed_tx()
            .then(|| bounded_commit_ban_reason(reject));
        let relay_reject = reject.is_allowed_relay() && !matches!(reject, Reject::Duplicated(_));
        let mut effects = Vec::new();
        if let Some(effect) = self.recent_reject_effect(tx_hash.clone(), reject) {
            effects.push(effect);
        }
        if let Some(reason) = &ban_reason {
            effects.push(TxPoolEffect::BanPeer {
                peer,
                duration: Duration::from_secs(MALFORMED_TX_BAN_SECONDS),
                reason: reason.clone(),
            });
        }
        if relay_reject {
            effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: tx_hash.clone(),
            }));
        }
        if let Some(reason) = ban_reason {
            const DEFAULT_BAN_TIME: Duration = Duration::from_secs(MALFORMED_TX_BAN_SECONDS);
            Self::report_malformed_peer_ban(peer, &reason);
            self.record_peer_ban(peer, DEFAULT_BAN_TIME);
        }
        self.publish_effects_class(effects, EffectClass::Remote)
            .await;
        if reject.is_malformed_tx() {
            self.remove_banned_peer_entries(peer).await;
        }
    }

    fn report_malformed_peer_ban(peer: PeerIndex, reason: &str) {
        #[cfg(not(feature = "with_sentry"))]
        let _ = (peer, reason);

        #[cfg(feature = "with_sentry")]
        sentry::with_scope(
            |scope| scope.set_fingerprint(Some(&["ckb-tx-pool", "receive-invalid-remote-tx"])),
            || {
                sentry::capture_message(
                    &format!(
                        "Ban peer {} for {} seconds, reason: {}",
                        peer, MALFORMED_TX_BAN_SECONDS, reason
                    ),
                    sentry::Level::Info,
                )
            },
        );
    }

    /// Install the internal fail-closed ban state before external network
    /// publication. Workers consult this map at every active boundary.
    pub(crate) fn record_peer_ban(&self, peer: PeerIndex, duration: Duration) {
        self.relay.banned_peers.record(peer, duration);
    }

    /// Revoke every non-committing Coordinator owner attributed to a banned
    /// peer in bounded, journaled slices.
    pub(crate) async fn remove_banned_peer_entries(&self, peer: PeerIndex) {
        // Revoke coordinator ownership in bounded slices. Active raw/verify
        // leases become stale immediately; an entry already inside the
        // write-locked commit boundary is allowed to settle and cannot make
        // the slice spin forever.
        const PEER_REMOVAL_SLICE: usize = 32;
        loop {
            let hashes = self
                .pipeline
                .kernel
                .read(|coordinator| coordinator.peer_hashes(peer, PEER_REMOVAL_SLICE));
            if hashes.is_empty() {
                break;
            }
            let preview = self.pipeline.kernel.read(|coordinator| {
                hashes
                    .iter()
                    .filter_map(|hash| coordinator.terminal_record(hash))
                    .collect::<Vec<_>>()
            });
            let preview_batch = self.pipeline_terminal_effects(&preview);
            if let Some(batch) = &preview_batch
                && self
                    .relay
                    .effects
                    .wait_capacity(batch.charge_bytes(), EffectClass::Remote)
                    .await
                    .is_err()
            {
                break;
            }
            let terminal = match self.pipeline.kernel.mutate(|coordinator| {
                let current = hashes
                    .iter()
                    .filter_map(|hash| coordinator.terminal_record(hash))
                    .collect::<Vec<_>>();
                let batch = self.pipeline_terminal_effects(&current);
                self.relay
                    .effects
                    .try_apply(batch, EffectClass::Remote, || {
                        coordinator.force_terminalize_many(
                            &hashes,
                            crate::component::pre_pool::TerminalDisposition::Removed,
                        )
                    })
            }) {
                Ok(Ok(records)) => records,
                Ok(Err(error)) => {
                    ckb_logger::error!("banned-peer owner cohort failed: {error:?}");
                    break;
                }
                Err(EffectJournalError::Full) => continue,
                Err(error) => {
                    ckb_logger::error!("banned-peer effect journal unavailable: {error:?}");
                    break;
                }
            };
            if terminal.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
}
