use super::{
    AuthorityChainUpdateError, AuthorityService, AuthorityVerificationCommand,
    RemoteIngressBatchProgress,
};
use crate::service::ChainReorgArgs;
use ckb_script::ChunkCommand;
use tokio::sync::watch;

impl AuthorityService {
    pub(crate) async fn apply_chain_update(
        &self,
        arguments: ChainReorgArgs,
    ) -> Result<(), AuthorityChainUpdateError> {
        let committed = self.commit_chain_update(arguments)?;
        self.publish_chain_observers(committed);
        Ok(())
    }
}

impl AuthorityVerificationCommand {
    pub(in crate::authority) fn subscribe(&self) -> watch::Receiver<ChunkCommand> {
        self.sender.subscribe()
    }
}

impl RemoteIngressBatchProgress {
    pub(crate) fn complete_for_foundation(completed: usize) -> Self {
        Self::complete(completed)
    }
}
