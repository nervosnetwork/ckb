use super::AuthorityVerificationCommand;
use ckb_script::ChunkCommand;
use tokio::sync::watch;

impl AuthorityVerificationCommand {
    pub(in crate::authority) fn subscribe(&self) -> watch::Receiver<ChunkCommand> {
        self.sender.subscribe()
    }
}
