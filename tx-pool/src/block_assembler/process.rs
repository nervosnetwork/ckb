use crate::service::{BlockAssemblerMessage, TxPoolService};
use std::sync::Arc;

pub(crate) async fn process(service: TxPoolService, message: &BlockAssemblerMessage) {
    match message {
        BlockAssemblerMessage::Pending => {
            if let Some(ref block_assembler) = service.block_assembler {
                block_assembler
                    .update_proposals(&service.pool.tx_pool)
                    .await;
            }
        }
        BlockAssemblerMessage::Proposed => {
            if let Some(ref block_assembler) = service.block_assembler
                && let Err(e) = block_assembler
                    .update_transactions(&service.pool.tx_pool)
                    .await
            {
                ckb_logger::error!("block_assembler update_transactions error {}", e);
            }
        }
        BlockAssemblerMessage::Uncle => {
            if let Some(ref block_assembler) = service.block_assembler {
                block_assembler.update_uncles().await;
            }
        }
        BlockAssemblerMessage::Reset(snapshot) => {
            // Management-triggered resets (e.g. `clear_pool`) must not be
            // dropped by the version check: the pool has already been cleared,
            // so the template must be rebuilt unconditionally. Miners must be
            // notified right away (same as the reorg path): otherwise they
            // keep mining a template built on the cleared pool until the next
            // Pending/Proposed message or interval batch.
            if let Some(ref block_assembler) = service.block_assembler {
                if let Err(e) = block_assembler.reset_template(Arc::clone(snapshot)).await {
                    ckb_logger::error!("block_assembler reset_template error {}", e);
                }
                block_assembler.notify().await;
            }
        }
    }
}
