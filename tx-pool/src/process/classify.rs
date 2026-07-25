//! Resolution for synchronous submissions and admission into the
//! kernel-owned asynchronous pipeline.

use super::{get_tx_status, make_pre_checked_tx, resolve_tx};
use crate::component::pre_pool::{ResolveLane, TerminalRecord};
use crate::error::Reject;
use crate::process::PreCheckedTx;
use crate::service::TxVerificationResult;
use crate::service::effects::{EffectBatch, EffectClass, TxPoolEffect, bounded_commit_ban_reason};
use crate::tx_source::TxSource;
use crate::util::{check_tx_fee, check_tx_fee_with_min_fee_rate, check_txid_collision};
use ckb_logger::error;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{TransactionView, cell::resolve_transaction};
use ckb_verification::cache::Completed;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::watch;

impl super::TxPoolService {
    fn stale_pipeline_reject() -> Reject {
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
    ) -> Result<super::submit::VerifySubmitOutcome, Reject> {
        // Local RPC and detached-block recovery bypass coordinator admission.
        // Materialize here so an accepted transaction never keeps the whole
        // caller-owned relay/block backing alive under a tx-sized charge.
        let tx = tx.into_compact();
        let epoch = self.current_pipeline_epoch()?;
        let tx_size = tx.data().serialized_size_in_block();
        let (ret, snapshot) = self.pre_check(&tx, tx_size).await;
        let PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
            resident_size,
        } = ret?;

        self.verify_and_submit_core(
            crate::resolved_tx::ResolvedTx {
                tx,
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

    pub(crate) async fn process_tx_direct(
        &self,
        tx: TransactionView,
        source: TxSource,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<Completed, Reject> {
        match self.process_tx_direct_outcome(tx, source, command_rx).await {
            Ok(super::submit::VerifySubmitOutcome::Committed(completed)) => Ok(completed),
            Ok(super::submit::VerifySubmitOutcome::Cleared) => Err(Self::stale_pipeline_reject()),
            Err(reject) => Err(reject),
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
            Err(reject) => Err(reject),
        };

        if let Err(reject) = &result {
            match source {
                TxSource::Remote { .. } => {
                    // Admission never established coordinator ownership, so
                    // this adapter owns the one definitive remote terminal.
                    self.reject_with_after_process(tx, source, reject.clone())
                        .await;
                }
                TxSource::Proposal if matches!(reject, Reject::Full(_)) => {
                    // Preserve the proposal-fetch backpressure contract: a
                    // transient local capacity failure releases the relayer's
                    // outstanding transaction filter without blaming a peer.
                    self.send_result_to_relayer(crate::service::TxVerificationResult::Reject {
                        tx_hash: tx.hash(),
                    })
                    .await;
                }
                TxSource::Local | TxSource::Proposal => {}
            }
        }
        result
    }

    async fn admit_pipeline_raw_at(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        stage: ResolveLane,
    ) -> Result<bool, Reject> {
        self.ensure_current(epoch)?;
        let tx_hash = tx.hash();
        let proposal_id = tx.proposal_short_id();
        // The early duplicate check is only a cheap filter. Admission itself
        // must share the universal TxPool -> coordinator boundary with commit:
        // otherwise a commit between the early check and this mutation leaves
        // the same transaction owned by both authorities.
        let tx_pool = self.pool.tx_pool.read().await;
        if tx_pool.get_tx_from_pool_by_hash(&tx_hash).is_some() {
            let effects = source
                .peer()
                .map(|peer| {
                    vec![TxPoolEffect::Relay(TxVerificationResult::Ok {
                        original_peer: Some(peer),
                        tx_hash,
                    })]
                })
                .unwrap_or_default();
            drop(tx_pool);
            self.publish_effects_class(effects, EffectClass::Remote)
                .await;
            return Ok(false);
        }
        if tx_pool.contains_proposal_id(&proposal_id) {
            // A short-id collision is not the same transaction and therefore
            // must never receive a successful duplicate settlement. The
            // proposal namespace is temporarily occupied, so expose retryable
            // backpressure instead of poisoning recent-reject state.
            return Err(Reject::Full(format!(
                "proposal short-id collision while admitting {tx_hash}"
            )));
        }
        let admitted = self
            .pipeline
            .kernel
            .admit_transaction(tx.clone(), source, epoch, stage);
        drop(tx_pool);
        match admitted {
            Ok(added) => Ok(added),
            Err(error) => {
                if !self.is_pipeline_epoch_current(epoch) {
                    Err(Self::stale_pipeline_reject())
                } else {
                    Err(self.pipeline.kernel.reject_or_fail(
                        "pipeline admission violated coordinator invariants",
                        error,
                    ))
                }
            }
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
