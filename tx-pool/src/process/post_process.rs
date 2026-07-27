use crate::constants::MALFORMED_TX_BAN_SECONDS;
use crate::error::Reject;
use crate::service::effects::{
    EffectCapacityWaitError, EffectClass, EffectJournalError, TxPoolEffect,
    bounded_commit_ban_reason,
};
use crate::service::{TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use ckb_logger::debug;
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::Byte32;
use std::time::Duration;

#[cfg(test)]
#[path = "tests/post_process_test_support.rs"]
mod test_support;

impl TxPoolService {
    pub(crate) async fn after_process(
        &self,
        tx: TransactionView,
        source: TxSource,
        reject: &Reject,
    ) {
        let epoch = match self.current_pipeline_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                self.fail_tx_pool_generation(
                    "post-process epoch unavailable",
                    &crate::process::TxPoolGenerationFault::Epoch(error),
                );
                return;
            }
        };
        self.after_process_at(tx, source, reject, epoch).await;
    }

    pub(crate) async fn after_process_at(
        &self,
        tx: TransactionView,
        source: TxSource,
        reject: &Reject,
        epoch: u64,
    ) {
        crate::metrics::record_rejection(reject);
        if !self.is_pipeline_epoch_current(epoch) {
            self.terminal_internal(tx, source).await;
            return;
        }
        let tx_hash = tx.hash();

        if matches!(
            reject,
            Reject::RBFRejected(..) | Reject::Resolve(OutPointError::Dead(_))
        ) {
            let tx_pool = self.pool.tx_pool.write().await;
            if self.is_pipeline_epoch_current(epoch)
                && tx_pool.pool_map.find_conflict_outpoint(&tx).is_some()
            {
                let raw = crate::component::pre_pool::PipelineRawTx::new(tx.clone(), source, epoch);
                let keys =
                    crate::component::pre_pool::conflict_dependency_keys(&tx, std::iter::empty());
                let owner = crate::component::pre_pool::historical_source(source);
                let expires_at = crate::component::pre_pool::historical_deadline(owner);
                let retention_error = self.pipeline.kernel.mutate_authoritative(|kernel| {
                    self.retain_optional_conflict(
                        kernel,
                        raw,
                        owner,
                        keys,
                        expires_at,
                        "post-process conflict retention failed",
                    )
                });
                if let Err(error) = retention_error {
                    self.fail_tx_pool_generation(
                        "post-process conflict retention failed",
                        &crate::process::TxPoolGenerationFault::PrePool(
                            error.into_unexpected_fault(),
                        ),
                    );
                }
            }
            drop(tx_pool);
        }

        match source {
            TxSource::Remote { peer, .. } => {
                debug!(
                    "after_process {} {} remote reject: {} ",
                    tx_hash, peer, reject
                );
                self.handle_remote_reject(&tx_hash, reject, peer).await;
            }
            TxSource::Local | TxSource::Proposal => {
                debug!("after_process {} reject: {} ", tx_hash, reject);
                if matches!(reject, Reject::Duplicated(_)) {
                    match self.publish_accepted_relay_result(tx_hash, None).await {
                        Ok(_) | Err(EffectJournalError::Closed) => {}
                        Err(error) => self.fail_tx_pool_generation(
                            "accepted duplicate relay publication failed",
                            &crate::process::TxPoolGenerationFault::Effect(error),
                        ),
                    }
                } else if let Some(effect) = self.recent_reject_effect(tx_hash, reject) {
                    self.publish_effects(vec![effect]).await;
                }
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
        self.after_process(tx, source, &reject).await;
    }

    /// Terminal routing for a transaction whose processing was cancelled or
    /// failed at an internal typed boundary. The transaction itself is not at fault, so
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
    /// keep flowing into the pool: sliced ingress revocation may race a job
    /// a worker has already checked out.
    pub(crate) fn is_recently_banned(&self, source: TxSource) -> bool {
        let Some(peer) = source.peer() else {
            return false;
        };
        self.relay.banned_peers.contains(peer)
    }
    /// Post-processing for a rejected remote transaction: ban the peer if the
    /// tx is malformed, relay the rejection if allowed, and record it in the
    /// recent-reject database if applicable.
    ///
    /// This is the single source of truth for remote terminal policy.
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

    /// Revoke every kernel owner attributed to a banned peer in bounded,
    /// journaled slices. Ready planning rechecks the same ban fence under the
    /// final authority guards, so only a commit that linearized before the
    /// marker may already be Accepted.
    pub(crate) async fn remove_banned_peer_entries(&self, peer: PeerIndex) {
        // Revoke kernel ownership in bounded slices. Active raw/verify
        // leases become stale immediately. A commit already past its final
        // fence check linearized before the ban; every later Ready Plan
        // terminalizes through the same immutable-ingress policy.
        const PEER_REMOVAL_SLICE: usize = 32;
        loop {
            let hashes = self
                .pipeline
                .kernel
                .read(|kernel| kernel.ingress_peer_hashes(peer, PEER_REMOVAL_SLICE));
            if hashes.is_empty() {
                break;
            }
            let preview = self.pipeline.kernel.read(|kernel| {
                hashes
                    .iter()
                    .filter_map(|hash| kernel.terminal_record(hash))
                    .collect::<Vec<_>>()
            });
            let preview_batch = self.pipeline_terminal_effects(&preview);
            if let Some(batch) = &preview_batch {
                match self
                    .relay
                    .effects
                    .wait_capacity(batch.charge_bytes(), EffectClass::Remote)
                    .await
                {
                    Ok(()) => {}
                    Err(EffectCapacityWaitError::Closed) => break,
                    Err(error) => {
                        self.fail_tx_pool_generation(
                            "peer-revocation effect capacity proof failed",
                            &crate::process::TxPoolGenerationFault::Effect(error.into()),
                        );
                        break;
                    }
                }
            }
            match self.pipeline.kernel.mutate_authoritative(
                |kernel| -> Result<_, crate::component::pre_pool::PrePoolError> {
                    // `hashes` is only an optimistic ingress-index snapshot.
                    // Rebind it under the sole kernel authority so removal can
                    // neither target a newer incarnation nor miss an owner
                    // whose current source was promoted after remote ingress.
                    let Some(plan) = kernel.plan_peer_revocation(peer, &hashes)? else {
                        return Ok(Ok(Vec::new()));
                    };
                    let batch = self.pipeline_terminal_effects(plan.records());
                    Ok(self
                        .relay
                        .effects
                        .try_apply(batch, EffectClass::Remote, || plan.apply()))
                },
            ) {
                Ok(Ok(records)) => records,
                Ok(Err(EffectJournalError::Full)) => continue,
                Ok(Err(EffectJournalError::Closed)) => break,
                Ok(Err(error)) => {
                    self.fail_tx_pool_generation(
                        "peer-revocation effect journal invariant failed",
                        &crate::process::TxPoolGenerationFault::Effect(error),
                    );
                    break;
                }
                Err(error) => {
                    self.fail_tx_pool_generation(
                        "banned-peer owner cohort transition failed",
                        &crate::process::TxPoolGenerationFault::PrePool(
                            error.into_unexpected_fault(),
                        ),
                    );
                    break;
                }
            };
            // Re-read the immutable ingress projection after every slice; a
            // stale snapshot may have lost or replaced every selected hash.
            tokio::task::yield_now().await;
        }
    }
}
