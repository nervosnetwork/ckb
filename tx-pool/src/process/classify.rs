//! Resolution for synchronous submissions and admission into the
//! coordinator-owned asynchronous pipeline.

use super::{get_tx_status, make_pre_checked_tx, resolve_tx};
use crate::component::pipeline_coordinator::{RawStage, TerminalRecord};
use crate::component::pipeline_runtime::{PipelineRawTx, coordinator_reject};
use crate::error::Reject;
use crate::process::PreCheckedTx;
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

    async fn ensure_current_or_terminal(
        &self,
        tx: &TransactionView,
        source: TxSource,
        epoch: u64,
    ) -> Result<(), Reject> {
        if self.is_pipeline_epoch_current(epoch) {
            Ok(())
        } else {
            self.terminal_internal(tx.clone(), source).await;
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
        let epoch = self.current_pipeline_epoch()?;
        let tx_size = tx.data().serialized_size_in_block();
        let (ret, snapshot) = self.pre_check(&tx, tx_size).await;
        let PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
        } = ret?;

        self.verify_and_submit_core(
            crate::resolved_tx::ResolvedTx {
                tx,
                rtx,
                status,
                fee,
                tx_size,
                pre_resolve_tip,
                snapshot,
                source,
                epoch,
                verified: None,
            },
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
        let epoch = self.current_pipeline_epoch()?;
        self.admit_pipeline_raw_at(tx, source, epoch, RawStage::PreCheck)
            .await
    }

    async fn admit_pipeline_raw_at(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        stage: RawStage,
    ) -> Result<bool, Reject> {
        self.ensure_current_or_terminal(&tx, source, epoch).await?;
        let permit = self
            .reserve_effects(Self::pipeline_terminal_effect_bytes(
                crate::constants::MAX_RBF_REPLACEMENT_CANDIDATES.saturating_add(1),
            ))
            .await
            .map_err(|error| {
                Reject::Internal(format!(
                    "pipeline admission effect reservation failed: {error:?}"
                ))
            })?;
        self.ensure_current_or_terminal(&tx, source, epoch).await?;
        match self.pipeline.runtime.admit_transaction_journaled(
            tx.clone(),
            source,
            epoch,
            stage,
            |records| {
                self.journal_pipeline_terminal_records(permit, records);
            },
        ) {
            Ok((added, _terminal)) => Ok(added),
            Err(error) => {
                if !self.is_pipeline_epoch_current(epoch) {
                    self.terminal_internal(tx, source).await;
                    Err(Self::stale_pipeline_reject())
                } else {
                    Err(coordinator_reject(error))
                }
            }
        }
    }

    pub(crate) fn pipeline_terminal_effect_bytes(max_records: usize) -> usize {
        max_records.saturating_mul(crate::service::effects::EFFECT_ENVELOPE_BYTES)
    }

    pub(crate) fn pipeline_outcome_effect_bytes(reject: Option<&Reject>) -> usize {
        let ban_reason = reject
            .filter(|reject| reject.is_malformed_tx())
            .map(|reject| format!("reject {reject}").len())
            .unwrap_or_default();
        crate::service::effects::EFFECT_ENVELOPE_BYTES
            .saturating_mul(2)
            .saturating_add(ban_reason)
    }

    /// Commit relayer terminal handoffs while the caller still owns the
    /// Coordinator (or outer TxPool→Coordinator) mutation lock. Terminal
    /// records are already detached from Coordinator residency; journaling
    /// them here prevents cancellation from creating a relayer filter leak.
    pub(crate) fn journal_pipeline_terminal_records(
        &self,
        permit: crate::service::effects::EffectPermit,
        records: &[TerminalRecord<PipelineRawTx>],
    ) {
        let mut effects = Vec::new();
        for record in records {
            match record.raw.authoritative_source(record.source) {
                Ok(source) if source.peer().is_some() => {
                    effects.push(crate::service::effects::TxPoolEffect::Relay(
                        crate::service::TxVerificationResult::Reject {
                            tx_hash: record.hash.clone(),
                        },
                    ));
                }
                Ok(_) => {}
                Err(reject) => {
                    ckb_logger::error!(
                        "cannot journal coordinator terminal record for {}: {}",
                        record.hash,
                        reject
                    );
                    effects.push(crate::service::effects::TxPoolEffect::Relay(
                        crate::service::TxVerificationResult::Reject {
                            tx_hash: record.hash.clone(),
                        },
                    ));
                }
            }
        }
        if let Err(error) = self.publish_reserved_effects(permit, effects) {
            panic!("reserved coordinator terminal journal failed: {error:?}");
        }
    }

    /// Journal the definitive outcome of one active raw/verify owner inside
    /// the same Coordinator transition that removes it. `reject == None`
    /// denotes an internal/cancellation terminal and therefore never records
    /// blame, while still releasing a remote relayer filter.
    pub(crate) fn journal_pipeline_outcome(
        &self,
        permit: crate::service::effects::EffectPermit,
        record: &TerminalRecord<PipelineRawTx>,
        reject: Option<&Reject>,
        mut tx_pool: Option<&mut crate::pool::TxPool>,
    ) -> Option<ckb_network::PeerIndex> {
        let source = match record.raw.authoritative_source(record.source) {
            Ok(source) => source,
            Err(error) => {
                ckb_logger::error!(
                    "cannot attribute coordinator terminal record {}: {}",
                    record.hash,
                    error
                );
                let effects = vec![crate::service::effects::TxPoolEffect::Relay(
                    crate::service::TxVerificationResult::Reject {
                        tx_hash: record.hash.clone(),
                    },
                )];
                if let Err(error) = self.publish_reserved_effects(permit, effects) {
                    panic!("reserved unattributed terminal journal failed: {error:?}");
                }
                return None;
            }
        };

        let mut effects = Vec::new();
        let mut banned_peer = None;
        if let Some(reject) = reject {
            if matches!(
                reject,
                Reject::RBFRejected(..)
                    | Reject::Resolve(ckb_types::core::error::OutPointError::Dead(_))
            ) && let Some(pool) = tx_pool.as_mut()
                && pool
                    .pool_map
                    .find_conflict_outpoint(&record.raw.tx)
                    .is_some()
            {
                pool.record_conflict(record.raw.tx.clone(), source);
            }
            if reject.should_recorded() {
                self.record_recent_reject(&record.hash, reject);
            }
            if let Some(peer) = source.peer() {
                if reject.is_malformed_tx() {
                    let reason = format!("reject {reject}");
                    let duration =
                        std::time::Duration::from_secs(crate::constants::MALFORMED_TX_BAN_SECONDS);
                    self.record_peer_ban(peer, duration);
                    effects.push(crate::service::effects::TxPoolEffect::BanPeer {
                        peer,
                        duration,
                        reason,
                    });
                    banned_peer = Some(peer);
                }
                if reject.is_allowed_relay() && !matches!(reject, Reject::Duplicated(_)) {
                    effects.push(crate::service::effects::TxPoolEffect::Relay(
                        crate::service::TxVerificationResult::Reject {
                            tx_hash: record.hash.clone(),
                        },
                    ));
                }
            }
        } else if source.peer().is_some() {
            effects.push(crate::service::effects::TxPoolEffect::Relay(
                crate::service::TxVerificationResult::Reject {
                    tx_hash: record.hash.clone(),
                },
            ));
        }
        if let Err(error) = self.publish_reserved_effects(permit, effects) {
            panic!("reserved pipeline outcome journal failed: {error:?}");
        }
        banned_peer
    }
}
