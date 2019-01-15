use ckb_core::block::Block;
use ckb_core::BlockNumber;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};

// An iterator over the entries of a `Chain`.
pub struct ChainIterator<CI> {
    shared: Shared<CI>,
    current: Option<Block>,
    tip: BlockNumber,
}

impl<CI: ChainIndex> ChainIterator<CI> {
    pub fn new(shared: Shared<CI>) -> Self {
        let current = shared.block_hash(0).and_then(|h| shared.block(&h));
        let tip = shared.tip().number;
        ChainIterator {
            shared,
            current,
            tip,
        }
    }

    pub fn len(&self) -> u64 {
        self.tip + 1
    }
}

impl<CI: ChainIndex> Iterator for ChainIterator<CI> {
    type Item = Block;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take();

        self.current = match current {
            Some(ref b) => {
                if let Some(block_hash) = self.shared.block_hash(b.header().number() + 1) {
                    self.shared.block(&block_hash)
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
            None => (0, Some(0)),
        }
    }
}
