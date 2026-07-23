//! Bounded re-entry of pool-conflict recoveries into the authoritative
//! coordinator.
//!
//! Missing-parent and in-flight conflict waiting are coordinator states. This
//! module only handles transactions recovered from the accepted pool's
//! historical conflict cache after their inputs become available again.

use crate::error::Reject;
use crate::service::TxVerificationResult;
use crate::service::effects::{EffectBatch, TxPoolEffect, callback_reject};
use crate::tx_source::TxSource;
use ckb_logger::warn;
use ckb_types::core::TransactionView;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// A normal recovery may wait briefly for coordinator capacity to be
/// reclaimed. Shutdown draining gets one non-blocking attempt so teardown is
/// not extended by this retry window.
const RECOVER_ENQUEUE_MAX_ATTEMPTS: usize = 40;

pub(crate) async fn enqueue_pipeline_recover_txs(
    runtime: Arc<crate::component::pipeline_runtime::PipelineRuntime>,
    jobs: Vec<crate::resolved_tx::ResolveJob>,
    cancel: &CancellationToken,
    relay: &crate::service::RelayState,
    recent_reject: Option<&Arc<crate::component::recent_reject::RecentReject>>,
    epoch: &crate::service::PipelineEpoch,
    observe_cancel: bool,
) {
    for job in jobs {
        let tx = job.tx.clone();
        let source = job.source;
        if !epoch.is_current(job.epoch) {
            recover_cancelled(relay, &tx, source).await;
            continue;
        }
        for attempt in 1..=RECOVER_ENQUEUE_MAX_ATTEMPTS {
            let permit = match relay
                .effects
                .reserve(
                    crate::service::TxPoolService::pipeline_terminal_effect_bytes(
                        crate::constants::MAX_RBF_REPLACEMENT_CANDIDATES.saturating_add(1),
                    ),
                )
                .await
            {
                Ok(permit) => permit,
                Err(error) => {
                    warn!("recovery effect reservation failed: {:?}", error);
                    recover_terminal(
                        relay,
                        recent_reject,
                        &tx,
                        source,
                        Reject::Internal(format!("recovery effect reservation failed: {error:?}")),
                    )
                    .await;
                    break;
                }
            };
            let result = runtime.admit_transaction_journaled(
                tx.clone(),
                source,
                job.epoch,
                crate::component::pipeline_coordinator::RawStage::Resolve,
                |records| journal_recovery_terminal_records(relay, permit, records),
            );
            match result {
                Ok((_added, _terminal)) => break,
                Err(error) => {
                    let reject = crate::component::pipeline_runtime::coordinator_reject(error);
                    if matches!(reject, Reject::Full(_))
                        && attempt < RECOVER_ENQUEUE_MAX_ATTEMPTS
                        && observe_cancel
                    {
                        tokio::select! {
                            _ = tokio::time::sleep(crate::resolve_mgr::LOCAL_ORPHAN_RETRY_DELAY) => {}
                            _ = cancel.cancelled() => {
                                recover_terminal(relay, recent_reject, &tx, source, reject).await;
                                break;
                            },
                        }
                    } else {
                        recover_terminal(relay, recent_reject, &tx, source, reject).await;
                        break;
                    }
                }
            }
        }
    }
}

fn journal_recovery_terminal_records(
    relay: &crate::service::RelayState,
    permit: crate::service::effects::EffectPermit,
    records: &[crate::component::pipeline_coordinator::TerminalRecord<
        crate::component::pipeline_runtime::PipelineRawTx,
        crate::resolved_tx::ResolvedTx,
        crate::component::pipeline_runtime::PipelineVerifiedTx,
    >],
) {
    let mut effects = Vec::new();
    for record in records {
        let source = record
            .raw
            .authoritative_source(record.source)
            .unwrap_or(TxSource::Local);
        if source.peer().is_some() {
            effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: record.hash.clone(),
            }));
        }
    }
    let result = match EffectBatch::new(effects) {
        Some(batch) => relay.effects.commit(permit, batch),
        None => {
            drop(permit);
            Ok(())
        }
    };
    if let Err(error) = result {
        panic!("reserved recovery terminal journal failed: {error:?}");
    }
}

async fn enqueue_effects(relay: &crate::service::RelayState, effects: Vec<TxPoolEffect>) {
    let Some(batch) = EffectBatch::new(effects) else {
        return;
    };
    if let Err(error) = relay.effects.enqueue(batch).await {
        warn!("recover terminal effect journal failed: {:?}", error);
    }
}

async fn recover_cancelled(
    relay: &crate::service::RelayState,
    tx: &TransactionView,
    source: TxSource,
) {
    if source.peer().is_some() {
        enqueue_effects(
            relay,
            vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: tx.hash(),
            })],
        )
        .await;
    }
}

async fn recover_terminal(
    relay: &crate::service::RelayState,
    recent_reject: Option<&Arc<crate::component::recent_reject::RecentReject>>,
    tx: &TransactionView,
    source: TxSource,
    reject: Reject,
) {
    let entry = crate::component::entry::TxEntry::dummy_resolve(
        tx.clone(),
        0,
        ckb_types::core::Capacity::zero(),
        tx.data().serialized_size_in_block(),
    );
    let mut effects = Vec::new();
    if relay.callbacks.reject.is_some() {
        effects.push(callback_reject(
            Arc::clone(&relay.callbacks),
            entry,
            reject.clone(),
        ));
    }
    if (source.peer().is_some() || reject.is_allowed_relay())
        && !matches!(reject, Reject::Duplicated(_))
    {
        effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
            tx_hash: tx.hash(),
        }));
    }
    if reject.should_recorded()
        && let Some(store) = recent_reject
    {
        if let Err(error) = store.put(&tx.hash(), reject.clone()) {
            warn!(
                "failed to record recovered reject {} {}: {}",
                tx.hash(),
                reject,
                error
            );
        }
    }
    enqueue_effects(relay, effects).await;
}
