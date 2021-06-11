//! TODO(doc): @quake
use crate::ChainStore;
use ckb_traits::{BlockEpoch, CellDataProvider, EpochProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{EpochExt, HeaderView},
    packed::{Byte32, OutPoint},
};

/// TODO(doc): @quake
pub struct DataLoaderWrapper<'a, T>(&'a T);
impl<'a, T: ChainStore<'a>> DataLoaderWrapper<'a, T> {
    /// TODO(doc): @quake
    pub fn new(source: &'a T) -> Self {
        DataLoaderWrapper(source)
    }
}

impl<'a, T: ChainStore<'a>> CellDataProvider for DataLoaderWrapper<'a, T> {
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<Bytes> {
        self.0.get_cell_data(out_point).map(|(data, _)| data)
    }

    fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<Byte32> {
        self.0.get_cell_data_hash(out_point)
    }
}

impl<'a, T: ChainStore<'a>> HeaderProvider for DataLoaderWrapper<'a, T> {
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        self.0.get_block_header(block_hash)
    }
}

impl<'a, T: ChainStore<'a>> EpochProvider for DataLoaderWrapper<'a, T> {
    fn get_epoch_ext(&self, header: &HeaderView) -> Option<EpochExt> {
        self.0
            .get_block_epoch_index(&header.hash())
            .and_then(|index| self.0.get_epoch_ext(&index))
    }

    fn get_block_epoch(&self, header: &HeaderView) -> Option<BlockEpoch> {
        self.get_epoch_ext(header).map(|epoch| {
            if header.number() != epoch.start_number() + epoch.length() - 1 {
                BlockEpoch::NonTailBlock { epoch }
            } else {
                let last_block_hash_in_previous_epoch = if epoch.is_genesis() {
                    self.0.get_block_hash(0).expect("genesis block stored")
                } else {
                    epoch.last_block_hash_in_previous_epoch()
                };
                let epoch_uncles_count = self
                    .0
                    .get_block_ext(&header.hash())
                    .expect("stored block ext")
                    .total_uncles_count
                    - self
                        .0
                        .get_block_ext(&last_block_hash_in_previous_epoch)
                        .expect("stored block ext")
                        .total_uncles_count;
                let epoch_duration_in_milliseconds = header.timestamp()
                    - self
                        .0
                        .get_block_header(&last_block_hash_in_previous_epoch)
                        .expect("stored block header")
                        .timestamp();

                BlockEpoch::TailBlock {
                    epoch,
                    epoch_uncles_count,
                    epoch_duration_in_milliseconds,
                }
            }
        })
    }
}
