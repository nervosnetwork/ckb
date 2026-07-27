use crate::service::{BlockAssemblerMessage, TxPoolService};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResetApply {
    Idle,
    Applied,
    Superseded,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResetNotification {
    NotifyBlank,
    SuppressUntilFull,
}

/// Apply the latest generation-tagged reset. Reorg refresh uses
/// `SuppressUntilFull` so miners observe the subsequent high-priority full
/// template once; the ordinary reset consumer uses `NotifyBlank` to publish
/// the blank management template immediately.
pub(crate) async fn process_reset(
    service: TxPoolService,
    notification: ResetNotification,
) -> ResetApply {
    let (Some(block_assembler), Some(pending)) = (
        service.block_assembler.as_ref(),
        service.relay.load_block_assembler_reset(),
    ) else {
        return ResetApply::Idle;
    };

    let prepared = match block_assembler
        .prepare_reset_template(pending.snapshot())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            ckb_logger::error!(
                "block_assembler reset preparation error; reset remains pending: {}",
                error
            );
            return if service.relay.block_assembler_reset.is_current(&pending) {
                ResetApply::Retry
            } else {
                ResetApply::Superseded
            };
        }
    };
    let applied = match block_assembler
        .publish_reset_template(prepared, &pending, &service.relay.block_assembler_reset)
        .await
    {
        Ok(applied) => applied,
        Err(error) => {
            ckb_logger::error!(
                "block_assembler reset publication error; reset remains pending: {}",
                error
            );
            return if service.relay.block_assembler_reset.is_current(&pending) {
                ResetApply::Retry
            } else {
                ResetApply::Superseded
            };
        }
    };
    if !applied {
        return ResetApply::Superseded;
    }
    if service.relay.block_assembler_reset_pending() {
        // A newer snapshot arrived after the exact publication. Its token is
        // still pending and is now the sole template authority.
        return ResetApply::Superseded;
    }
    // Reset is an authoritative replacement. Any optimistic partial update
    // which completed against the old template is therefore reissued
    // level-wise, including an uncle update that raced with reset preparation.
    service.journal_block_assembler_full_reconcile();
    if matches!(notification, ResetNotification::NotifyBlank) {
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
                return match block_assembler
                    .update_proposals(&service.pool.tx_pool)
                    .await
                {
                    Ok(applied) => applied,
                    Err(error) => {
                        ckb_logger::error!("block_assembler update_proposals error {}", error);
                        false
                    }
                };
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
                return match (uncles_applied, proposals_applied) {
                    (Ok(uncles), Ok(proposals)) => uncles && proposals,
                    (Err(error), _) | (_, Err(error)) => {
                        ckb_logger::error!("block_assembler uncle/proposal update error {}", error);
                        false
                    }
                };
            }
        }
        BlockAssemblerMessage::Reset => {
            // Management-triggered resets (e.g. `clear_pool`) must not be
            // dropped by the version check: the pool has already been cleared,
            // so the template must be rebuilt unconditionally. Miners must be
            // notified right away (same as the reorg path): otherwise they
            // keep mining a template built on the cleared pool until the next
            // Pending/Proposed message or interval batch.
            return process_reset(service, ResetNotification::NotifyBlank).await
                != ResetApply::Retry;
        }
    }
    true
}
