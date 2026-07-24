use crate::service::{BlockAssemblerMessage, TxPoolService};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResetApply {
    Idle,
    Applied,
    Superseded,
    Retry,
}

/// Apply the latest generation-tagged reset. Reorg refresh uses
/// `notify = false` so miners observe the subsequent high-priority full
/// template once; the ordinary reset consumer publishes the blank management
/// template immediately.
pub(crate) async fn process_reset(service: TxPoolService, notify: bool) -> ResetApply {
    let (Some(block_assembler), Some((generation, snapshot))) = (
        service.block_assembler.as_ref(),
        service.relay.load_block_assembler_reset(),
    ) else {
        return ResetApply::Idle;
    };
    if let Err(e) = block_assembler.reset_template(Arc::clone(&snapshot)).await {
        ckb_logger::error!(
            "block_assembler reset_template error; reset remains pending: {}",
            e
        );
        // If a newer generation arrived (or another serialized consumer
        // completed this one), retrying the stale generation is unnecessary.
        return if service
            .relay
            .load_block_assembler_reset()
            .is_some_and(|(pending, _)| pending == generation)
        {
            ResetApply::Retry
        } else {
            ResetApply::Superseded
        };
    }
    service.relay.complete_block_assembler_reset(generation);
    if service.relay.block_assembler_reset_pending() {
        // A newer snapshot arrived during the rebuild. Do not publish the old
        // template or unblock ordinary deltas.
        return ResetApply::Superseded;
    }
    if notify {
        block_assembler.notify().await;
    }
    ResetApply::Applied
}

/// Apply one template journal item. `false` means the authoritative item must
/// remain pending and be retried; callers acknowledge only `true` results.
pub(crate) async fn process(service: TxPoolService, message: &BlockAssemblerMessage) -> bool {
    match message {
        BlockAssemblerMessage::Pending => {
            if let Some(ref block_assembler) = service.block_assembler {
                return block_assembler
                    .update_proposals(&service.pool.tx_pool)
                    .await;
            }
        }
        BlockAssemblerMessage::Proposed => {
            if let Some(ref block_assembler) = service.block_assembler {
                return match block_assembler
                    .update_transactions(&service.pool.tx_pool)
                    .await
                {
                    Ok(applied) => applied,
                    Err(e) => {
                        ckb_logger::error!("block_assembler update_transactions error {}", e);
                        false
                    }
                };
            }
        }
        BlockAssemblerMessage::Uncle => {
            // Uncle and proposal selection must be refreshed together:
            // top-level Pending proposals take priority, and any candidate
            // uncle carrying the same short id is filtered from the template.
            if let Some(ref block_assembler) = service.block_assembler {
                let uncles_applied = block_assembler.update_uncles().await;
                let proposals_applied = block_assembler
                    .update_proposals(&service.pool.tx_pool)
                    .await;
                return uncles_applied && proposals_applied;
            }
        }
        BlockAssemblerMessage::Reset => {
            // Management-triggered resets (e.g. `clear_pool`) must not be
            // dropped by the version check: the pool has already been cleared,
            // so the template must be rebuilt unconditionally. Miners must be
            // notified right away (same as the reorg path): otherwise they
            // keep mining a template built on the cleared pool until the next
            // Pending/Proposed message or interval batch.
            return process_reset(service, true).await != ResetApply::Retry;
        }
    }
    true
}
