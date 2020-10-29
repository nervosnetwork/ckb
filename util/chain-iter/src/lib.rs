//! TODO(doc): @quake
use ckb_store::ChainStore;
use ckb_types::{core::BlockNumber, core::BlockView};

/// TODO(doc): @quake
// An iterator over the entries of a `Chain`.
pub struct ChainIterator<'a, S: ChainStore<'a>> {
    store: &'a S,
    current: Option<BlockView>,
    tip: BlockNumber,
}

impl<'a, S: ChainStore<'a>> ChainIterator<'a, S> {
    /// TODO(doc): @quake
    pub fn new(store: &'a S) -> Self {
        let current = store.get_block_hash(0).and_then(|h| store.get_block(&h));
        let tip = store.get_tip_header().expect("store inited").number();
        ChainIterator {
            store,
            current,
            tip,
        }
    }

    /// TODO(doc): @quake
    pub fn len(&self) -> u64 {
        self.tip + 1
    }

    /// TODO(doc): @quake
    // we always have genesis, this function may be meaningless
    // but for convention, mute len-without-is-empty lint
    pub fn is_empty(&self) -> bool {
        false
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
            //The default implementation returns (0, None) which is correct for any iterator.
            None => (0, None),
        }
    }
}
