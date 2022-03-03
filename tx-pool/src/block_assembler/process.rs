use crate::service::{BlockAssemblerMessage, TxPoolService};

pub(crate) async fn process(service: TxPoolService, message: &BlockAssemblerMessage) {
    match message {
        BlockAssemblerMessage::Pending => {
            if let Some(ref block_assembler) = service.block_assembler {
                block_assembler.update_proposals(&service.tx_pool).await;
            }
        }
        BlockAssemblerMessage::Proposed => {
            if let Some(ref block_assembler) = service.block_assembler {
                block_assembler.update_transactions(&service.tx_pool).await;
            }
        }
        BlockAssemblerMessage::Uncle => {
            if let Some(ref block_assembler) = service.block_assembler {
                block_assembler.update_uncles().await;
            }
        }
    }
}
