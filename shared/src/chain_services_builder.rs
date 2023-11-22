use crate::types::VerifyFailedBlockInfo;
use crate::Shared;
use ckb_proposal_table::ProposalTable;

pub struct ChainServicesBuilder {
    pub shared: Shared,
    pub proposal_table: ProposalTable,
    pub verify_failed_blocks_tx: tokio::sync::mpsc::UnboundedSender<VerifyFailedBlockInfo>,
}

impl ChainServicesBuilder {
    pub fn new(
        shared: Shared,
        proposal_table: ProposalTable,
        verify_failed_blocks_tx: tokio::sync::mpsc::UnboundedSender<VerifyFailedBlockInfo>,
    ) -> Self {
        ChainServicesBuilder {
            shared,
            proposal_table,
            verify_failed_blocks_tx,
        }
    }
}
