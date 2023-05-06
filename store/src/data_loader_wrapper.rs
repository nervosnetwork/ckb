//! TODO(doc): @quake
use crate::ChainStore;
use ckb_traits::{CellDataProvider, EpochProvider, ExtensionProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{BlockExt, BlockNumber, EpochExt, HeaderView},
    packed::{self, Byte32, OutPoint},
};
use std::sync::Arc;

/// DataLoaderWrapper wrap`ChainStore`
/// impl `HeaderProvider` `CellDataProvider` `EpochProvider`
pub struct DataLoaderWrapper<T>(Arc<T>);

// auto derive don't work
impl<T> Clone for DataLoaderWrapper<T> {
    fn clone(&self) -> Self {
        DataLoaderWrapper(Arc::clone(&self.0))
    }
}

/// Auto transform Arc wrapped `ChainStore` to `DataLoaderWrapper`
pub trait AsDataLoader<T> {
    /// Return arc cloned DataLoaderWrapper
    fn as_data_loader(&self) -> DataLoaderWrapper<T>;
}

impl<T> AsDataLoader<T> for Arc<T>
where
    T: ChainStore,
{
    fn as_data_loader(&self) -> DataLoaderWrapper<T> {
        DataLoaderWrapper(Arc::clone(self))
    }
}

impl<T> CellDataProvider for DataLoaderWrapper<T>
where
    T: ChainStore,
{
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<Bytes> {
        ChainStore::get_cell_data(self.0.as_ref(), out_point).map(|(data, _)| data)
    }

    fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<Byte32> {
        ChainStore::get_cell_data_hash(self.0.as_ref(), out_point)
    }
}

impl<T> HeaderProvider for DataLoaderWrapper<T>
where
    T: ChainStore,
{
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        ChainStore::get_block_header(self.0.as_ref(), block_hash)
    }
}

impl<T> EpochProvider for DataLoaderWrapper<T>
where
    T: ChainStore,
{
    fn get_epoch_ext(&self, header: &HeaderView) -> Option<EpochExt> {
        ChainStore::get_block_epoch_index(self.0.as_ref(), &header.hash())
            .and_then(|index| ChainStore::get_epoch_ext(self.0.as_ref(), &index))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Option<Byte32> {
        ChainStore::get_block_hash(self.0.as_ref(), number)
    }

    fn get_block_ext(&self, block_hash: &Byte32) -> Option<BlockExt> {
        ChainStore::get_block_ext(self.0.as_ref(), block_hash)
    }

    fn get_block_header(&self, hash: &Byte32) -> Option<HeaderView> {
        ChainStore::get_block_header(self.0.as_ref(), hash)
    }
}

impl<T> ExtensionProvider for DataLoaderWrapper<T>
where
    T: ChainStore,
{
    fn get_block_extension(&self, hash: &Byte32) -> Option<packed::Bytes> {
        ChainStore::get_block_extension(self.0.as_ref(), hash)
    }
}

/// Borrowed DataLoaderWrapper with lifetime
pub struct BorrowedDataLoaderWrapper<'a, T>(&'a T);
impl<'a, T: ChainStore> BorrowedDataLoaderWrapper<'a, T> {
    /// Construct new BorrowedDataLoaderWrapper
    pub fn new(source: &'a T) -> Self {
        BorrowedDataLoaderWrapper(source)
    }
}

impl<'a, T: ChainStore> CellDataProvider for BorrowedDataLoaderWrapper<'a, T> {
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<Bytes> {
        self.0.get_cell_data(out_point).map(|(data, _)| data)
    }

    fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<Byte32> {
        self.0.get_cell_data_hash(out_point)
    }
}

impl<'a, T: ChainStore> HeaderProvider for BorrowedDataLoaderWrapper<'a, T> {
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        self.0.get_block_header(block_hash)
    }
}

impl<'a, T: ChainStore> EpochProvider for BorrowedDataLoaderWrapper<'a, T> {
    fn get_epoch_ext(&self, header: &HeaderView) -> Option<EpochExt> {
        ChainStore::get_block_epoch_index(self.0, &header.hash())
            .and_then(|index| ChainStore::get_epoch_ext(self.0, &index))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Option<Byte32> {
        ChainStore::get_block_hash(self.0, number)
    }

    fn get_block_ext(&self, block_hash: &Byte32) -> Option<BlockExt> {
        ChainStore::get_block_ext(self.0, block_hash)
    }

    fn get_block_header(&self, hash: &Byte32) -> Option<HeaderView> {
        ChainStore::get_block_header(self.0, hash)
    }
}

impl<'a, T: ChainStore> ExtensionProvider for BorrowedDataLoaderWrapper<'a, T> {
    fn get_block_extension(&self, hash: &Byte32) -> Option<packed::Bytes> {
        ChainStore::get_block_extension(self.0, hash)
    }
}
