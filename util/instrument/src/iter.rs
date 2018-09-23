use ckb_chain::chain::ChainProvider;
use ckb_core::block::IndexedBlock;
use ckb_core::BlockNumber;

// An iterator over the entries of a `Chain`.
pub struct ChainIterator<'a, P: 'a> {
    chain: &'a P,
    current: Option<IndexedBlock>,
    tip: BlockNumber,
}

impl<'a, P: ChainProvider> ChainIterator<'a, P> {
    pub fn new(chain: &'a P) -> Self {
        ChainIterator {
            chain,
            current: chain.block_hash(0).and_then(|h| chain.block(&h)),
            tip: chain.tip_header().read().header.number,
        }
    }

    pub fn len(&self) -> u64 {
        self.tip + 1
    }
}

impl<'a, P: ChainProvider> Iterator for ChainIterator<'a, P> {
    type Item = IndexedBlock;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take();

        self.current = match current {
            Some(ref b) => {
                if let Some(block_hash) = self.chain.block_hash(b.number() + 1) {
                    self.chain.block(&block_hash)
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
            Some(ref b) => (1, Some((self.tip - b.number() + 1) as usize)),
            None => (0, Some(0)),
        }
    }
}
