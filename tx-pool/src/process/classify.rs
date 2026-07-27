//! Resolution for synchronous submissions and admission into the
//! kernel-owned asynchronous pipeline.

use super::{get_tx_status, make_pre_checked_tx, resolve_tx};
use crate::component::pre_pool::{PrePoolAdmissionError, ResolveLane, TerminalRecord};
use crate::error::Reject;
use crate::process::{PreCheckedTx, TxPoolGenerationFault};
use crate::service::TxVerificationResult;
use crate::service::effects::{EffectBatch, TxPoolEffect, bounded_commit_ban_reason};
use crate::tx_source::TxSource;
use crate::util::{check_tx_fee, check_tx_fee_with_min_fee_rate, check_txid_collision};
use ckb_logger::error;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{TransactionView, cell::resolve_transaction};
#[cfg(test)]
use ckb_verification::cache::Completed;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::watch;

/// Closed ingress failure domain. Only `Rejected` may enter transaction/peer
/// terminal policy; administrative invalidation releases relay state without
/// blame, and a structural fault converges the whole relay generation before
/// fail-stop.
enum PipelineAdmissionFailure {
    Rejected(Reject),
    Invalidated(Reject),
    /// The external ban marker won the admission race. The bounded peer
    /// removal transaction already owns any required relayer Reject, so the
    /// adapter must not publish a second terminal effect.
    Revoked(Reject),
    Fault(TxPoolGenerationFault),
}

#[cfg(test)]
#[path = "tests/classify_seam.rs"]
mod test_seam;

impl super::TxPoolService {
    pub(crate) fn stale_pipeline_reject() -> Reject {
        Reject::Internal("tx-pool pipeline generation invalidated by clear".to_string())
    }

    fn ensure_current(&self, epoch: u64) -> Result<(), Reject> {
        if self.is_pipeline_epoch_current(epoch) {
            Ok(())
        } else {
            Err(Self::stale_pipeline_reject())
        }
    }

    pub(crate) async fn pre_check(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        let (collision, snapshot) = self
            .read_tx_pool_with_snapshot(|tx_pool, _snapshot| {
                check_txid_collision(tx_pool, tx).err()
            })
            .await;
        if let Some(reject) = collision {
            return (Err(reject), snapshot);
        }

        let short_id = tx.proposal_short_id();
        let mut seen_inputs =
            HashSet::with_capacity(tx.inputs().len().saturating_add(tx.cell_deps().len()));
        match resolve_transaction(
            tx.clone(),
            &mut seen_inputs,
            snapshot.as_ref(),
            snapshot.as_ref(),
        ) {
            Ok(rtx) => {
                let rtx = Arc::new(rtx);
                let fee = match check_tx_fee_with_min_fee_rate(
                    &snapshot,
                    &rtx,
                    tx_size,
                    self.pool.tx_pool_config.min_fee_rate,
                ) {
                    Ok(fee) => fee,
                    Err(reject) => return (Err(reject), snapshot),
                };
                let status = get_tx_status(&snapshot, &short_id);
                (
                    Ok(make_pre_checked_tx(
                        snapshot.tip_hash(),
                        rtx,
                        status,
                        fee,
                        tx_size,
                    )),
                    snapshot,
                )
            }
            Err(OutPointError::Unknown(_)) => self.pre_check_with_pool_lock(tx, tx_size).await,
            Err(err) => (Err(Reject::Resolve(err)), snapshot),
        }
    }

    async fn pre_check_with_pool_lock(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        let (ret, snapshot) = self
            .read_tx_pool_with_snapshot(|tx_pool, snapshot| {
                let tip_hash = snapshot.tip_hash();
                check_txid_collision(tx_pool, tx)?;
                match resolve_tx(tx_pool, &snapshot, tx.clone(), false) {
                    Ok((rtx, status)) => {
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        Ok(make_pre_checked_tx(tip_hash, rtx, status, fee, tx_size))
                    }
                    Err(Reject::Resolve(OutPointError::Dead(out))) => {
                        let (rtx, status) = resolve_tx(tx_pool, &snapshot, tx.clone(), true)?;
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        if tx_pool.pool_map.find_conflict_outpoint(tx).is_none() {
                            error!(
                                "{} is resolved as Dead, but there is no direct conflicted tx",
                                rtx.transaction.proposal_short_id()
                            );
                            return Err(Reject::Resolve(OutPointError::Dead(out)));
                        }
                        Ok(make_pre_checked_tx(tip_hash, rtx, status, fee, tx_size))
                    }
                    Err(err) => Err(err),
                }
            })
            .await;
        (ret, snapshot)
    }

    pub(crate) async fn process_tx_direct_outcome(
        &self,
        tx: TransactionView,
        source: TxSource,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<super::submit::VerifySubmitOutcome, super::submit::SubmissionError> {
        // Local RPC and detached-block recovery bypass coordinator admission.
        // Materialize here so an accepted transaction never keeps the whole
        // caller-owned relay/block backing alive under a tx-sized charge.
        let tx = tx.into_compact();
        let epoch = self.current_pipeline_epoch().map_err(|error| {
            super::submit::SubmissionError::Fault(super::submit::PipelineCommitFault::Epoch(error))
        })?;
        let tx_size = tx.data().serialized_size_in_block();
        let (ret, snapshot) = self.pre_check(&tx, tx_size).await;
        let PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
            resident_size,
        } = ret.map_err(super::submit::SubmissionError::Rejected)?;

        self.verify_and_submit_core(
            crate::resolved_tx::ResolvedTx {
                rtx,
                status,
                fee,
                tx_size,
                resident_size,
                pre_resolve_tip,
                source,
                epoch,
            },
            snapshot,
            command_rx,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn process_tx_direct(
        &self,
        tx: TransactionView,
        source: TxSource,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<Completed, Reject> {
        match self.process_tx_direct_outcome(tx, source, command_rx).await {
            Ok(super::submit::VerifySubmitOutcome::Committed(completed)) => Ok(completed),
            Ok(super::submit::VerifySubmitOutcome::Cleared) => Err(Self::stale_pipeline_reject()),
            Err(super::submit::SubmissionError::Rejected(reject)) => Err(reject),
            Err(super::submit::SubmissionError::Fault(fault)) => {
                self.fail_tx_pool_generation(
                    "direct transaction processing failed",
                    &TxPoolGenerationFault::Commit(fault),
                );
                Err(Reject::Internal(
                    "tx-pool transaction processing generation failed".to_owned(),
                ))
            }
        }
    }

    pub(crate) async fn classify_and_enqueue_tx_spawn(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        let result = match self.current_pipeline_epoch() {
            Ok(epoch) => {
                self.admit_pipeline_raw_at(tx.clone(), source, epoch, ResolveLane::Ingress)
                    .await
            }
            Err(error) => Err(PipelineAdmissionFailure::Fault(
                TxPoolGenerationFault::Epoch(error),
            )),
        };

        self.finish_pipeline_admission(tx, source, result).await
    }

    async fn finish_pipeline_admission(
        &self,
        tx: TransactionView,
        source: TxSource,
        result: Result<bool, PipelineAdmissionFailure>,
    ) -> Result<bool, Reject> {
        match result {
            Ok(admitted) => Ok(admitted),
            Err(PipelineAdmissionFailure::Rejected(reject)) => {
                match source {
                    TxSource::Remote { .. } => {
                        // Admission never established coordinator ownership,
                        // so this adapter owns the definitive remote terminal.
                        self.reject_with_after_process(tx, source, reject.clone())
                            .await;
                    }
                    TxSource::Proposal if matches!(reject, Reject::Full(_)) => {
                        // Proposal-fetch capacity is retryable and carries no
                        // peer blame, but its outstanding filter must release.
                        self.send_result_to_relayer(TxVerificationResult::Reject {
                            tx_hash: tx.hash(),
                        })
                        .await;
                    }
                    TxSource::Local | TxSource::Proposal => {}
                }
                Err(reject)
            }
            Err(PipelineAdmissionFailure::Invalidated(reject)) => {
                self.terminal_internal(tx, source).await;
                Err(reject)
            }
            Err(PipelineAdmissionFailure::Revoked(reject)) => Err(reject),
            Err(PipelineAdmissionFailure::Fault(error)) => {
                self.fail_tx_pool_generation("pipeline admission invariant failed", &error);
                Err(Reject::Internal(
                    "tx-pool pipeline admission generation failed".to_owned(),
                ))
            }
        }
    }

    async fn admit_pipeline_raw_at(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        stage: ResolveLane,
    ) -> Result<bool, PipelineAdmissionFailure> {
        let admission_source =
            crate::component::pre_pool::PipelineAdmissionSource::from_tx_source(source)
                .ok_or_else(|| {
                    PipelineAdmissionFailure::Rejected(Reject::Internal(
                        "local submissions cannot enter the asynchronous pipeline".to_string(),
                    ))
                })?;
        let tx_hash = tx.hash();
        let proposal_id = tx.proposal_short_id();
        // The early duplicate check is only a cheap filter. Admission itself
        // must share the universal TxPool -> coordinator boundary with commit:
        // otherwise a commit between the early check and this mutation leaves
        // the same transaction owned by both authorities.
        loop {
            self.ensure_current(epoch)
                .map_err(PipelineAdmissionFailure::Invalidated)?;
            let tx_pool = self.pool.tx_pool.read().await;
            if tx_pool.get_tx_from_pool_by_hash(&tx_hash).is_some() {
                let Some(peer) = source.peer() else {
                    return Ok(false);
                };
                drop(tx_pool);
                match self
                    .publish_accepted_relay_result(tx_hash.clone(), Some(peer))
                    .await
                {
                    Ok(true) => return Ok(false),
                    // The accepted owner disappeared before the capability
                    // was acquired. Re-enter the complete ownership decision
                    // instead of acknowledging stale membership.
                    Ok(false) => continue,
                    Err(crate::service::effects::EffectJournalError::Closed) => {
                        return Err(PipelineAdmissionFailure::Invalidated(
                            Self::stale_pipeline_reject(),
                        ));
                    }
                    Err(error) => {
                        return Err(PipelineAdmissionFailure::Fault(
                            TxPoolGenerationFault::Effect(error),
                        ));
                    }
                }
            }
            if tx_pool.contains_proposal_id(&proposal_id) {
                // A short-id collision is not the same transaction and therefore
                // must never receive a successful duplicate settlement. The
                // proposal namespace is temporarily occupied, so expose retryable
                // backpressure instead of poisoning recent-reject state.
                return Err(PipelineAdmissionFailure::Rejected(Reject::Full(format!(
                    "proposal short-id collision while admitting {tx_hash}"
                ))));
            }
            let admitted =
                self.pipeline
                    .kernel
                    .admit_transaction(tx.clone(), admission_source, epoch, stage);
            drop(tx_pool);
            return match admitted {
                Ok(added) => {
                    if let Some(peer) = source.peer()
                        && self.relay.banned_peers.contains(peer)
                    {
                        // Either the ban's own slice observes this owner or
                        // this post-admission edge does. Ready planning also
                        // checks the same marker, so the owner cannot cross
                        // into Accepted between those two bounded removals.
                        self.remove_banned_peer_entries(peer).await;
                        Err(PipelineAdmissionFailure::Revoked(Reject::Internal(
                            "remote ingress invalidated by peer ban".to_owned(),
                        )))
                    } else {
                        Ok(added)
                    }
                }
                Err(_) if !self.is_pipeline_epoch_current(epoch) => Err(
                    PipelineAdmissionFailure::Invalidated(Self::stale_pipeline_reject()),
                ),
                Err(PrePoolAdmissionError::Public(error)) => {
                    Err(PipelineAdmissionFailure::Rejected(
                        crate::component::pre_pool::pre_pool_reject(error),
                    ))
                }
                Err(PrePoolAdmissionError::Fault(fault)) => Err(PipelineAdmissionFailure::Fault(
                    TxPoolGenerationFault::PrePool(fault),
                )),
            };
        }
    }

    /// Build the immutable remote-settlement batch before the matching total
    /// coordinator transition. The batch is later passed to the journal's
    /// `try_apply`, so capacity and ownership change share one inner critical
    /// section without a reservation token.
    pub(crate) fn pipeline_terminal_effects(
        &self,
        records: &[TerminalRecord],
    ) -> Option<EffectBatch> {
        let mut effects = Vec::new();
        for record in records {
            if record.raw.ingress_peer().is_some() {
                effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: record.hash.clone(),
                }));
            }
        }
        EffectBatch::new(effects)
    }

    /// Journal the definitive outcome of one active raw/verify owner inside
    /// the same Coordinator transition that removes it. `reject == None`
    /// denotes an internal/cancellation terminal and therefore never records
    /// blame, while still releasing a remote relayer filter.
    pub(crate) fn pipeline_outcome_effects(
        &self,
        record: &TerminalRecord,
        reject: Option<&Reject>,
    ) -> (
        Option<EffectBatch>,
        Option<(ckb_network::PeerIndex, std::time::Duration)>,
    ) {
        let ingress_peer = record.raw.ingress_peer();
        let blame_peer = record.raw.blame_peer();

        let mut effects = Vec::new();
        let mut peer_ban = None;
        if let Some(reject) = reject {
            if let Some(effect) = self.recent_reject_effect(record.hash.clone(), reject) {
                effects.push(effect);
            }
            if reject.is_malformed_tx()
                && let Some(peer) = blame_peer
            {
                let reason = bounded_commit_ban_reason(reject);
                let duration =
                    std::time::Duration::from_secs(crate::constants::MALFORMED_TX_BAN_SECONDS);
                effects.push(TxPoolEffect::BanPeer {
                    peer,
                    duration,
                    reason,
                });
                peer_ban = Some((peer, duration));
            }
            if ingress_peer.is_some()
                && reject.is_allowed_relay()
                && !matches!(reject, Reject::Duplicated(_))
            {
                effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: record.hash.clone(),
                }));
            }
        } else if ingress_peer.is_some() {
            effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: record.hash.clone(),
            }));
        }
        (EffectBatch::new(effects), peer_ban)
    }
}
