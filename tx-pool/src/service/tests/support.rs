use super::*;

impl ChainReorgPayloadLimit {
    pub(crate) const fn for_test(bytes: usize) -> Self {
        Self(bytes)
    }
}

impl ChainReorgArgs {
    pub(crate) fn for_test(
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        snapshot: Arc<Snapshot>,
    ) -> Self {
        let command = Self::bounded(
            detached_blocks,
            attached_blocks,
            snapshot,
            ChainReorgPayloadLimit::from_config(&TxPoolConfig::default()).unwrap(),
        );
        assert!(
            command.fork().is_some(),
            "test fork must fit the ingress bound"
        );
        command
    }

    pub(crate) fn into_fork(self) -> Option<ChainFork> {
        self.fork
    }
}
