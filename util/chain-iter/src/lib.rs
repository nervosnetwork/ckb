use ckb_store::ChainStore;
use ckb_types::{core::BlockNumber, core::BlockView};

// An iterator over the entries of a `Chain`.
pub struct ChainIterator<'a, S: ChainStore<'a>> {
    store: &'a S,
    current: Option<BlockView>,
    tip: BlockNumber,
}

impl<'a, S: ChainStore<'a>> ChainIterator<'a, S> {
    pub fn new(store: &'a S) -> Self {
        let current = store.get_block_hash(0).and_then(|h| store.get_block(&h));
        let tip = store.get_tip_header().expect("store inited").number();
        ChainIterator {
            store,
            current,
            tip,
        }
    }

    pub fn len(&self) -> u64 {
        self.tip + 1
    }
}

impl<'a, S: ChainStore<'a>> Iterator for ChainIterator<'a, S> {
    type Item = BlockView;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take();

        self.current = match current {
            Some(ref b) => {
                if let Some(block_hash) = self.store.get_block_hash(b.header().number() + 1) {
                    self.store.get_block(&block_hash)
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
