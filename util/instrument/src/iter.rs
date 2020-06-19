use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_types::{core::BlockNumber, core::BlockView};
use std::sync::Arc;

// An iterator over the entries of a `Chain`.
pub struct ChainIterator {
    snapshot: Arc<Snapshot>,
    current: Option<BlockView>,
    tip: BlockNumber,
}

impl ChainIterator {
    pub fn new(shared: &Shared) -> Self {
        let snapshot = Arc::clone(&shared.snapshot());
        let current = snapshot
            .get_block_hash(0)
            .and_then(|h| snapshot.get_block(&h));
        let tip = snapshot.tip_number();
        ChainIterator {
            snapshot,
            current,
            tip,
        }
    }

    pub fn size(&self) -> u64 {
        self.tip + 1
    }
}

impl Iterator for ChainIterator {
    type Item = BlockView;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take();

        self.current = match current {
            Some(ref b) => {
                if let Some(block_hash) = self.snapshot.get_block_hash(b.header().number() + 1) {
                    self.snapshot.get_block(&block_hash)
                } else {
                    None
                }
            }
            None => None,
        };
        current
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self.current {
            Some(ref b) => (1, Some((self.tip - b.header().number() + 1) as usize)),
            None => (0, None),
        }
    }
}
