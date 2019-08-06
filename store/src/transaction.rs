use crate::store::ChainStore;
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL_SET, COLUMN_EPOCH, COLUMN_INDEX,
    COLUMN_META, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES, META_CURRENT_EPOCH_KEY,
    META_TIP_HEADER_KEY,
};
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus, HeaderProvider, HeaderStatus};
use ckb_core::extras::{BlockExt, EpochExt, TransactionInfo};
use ckb_core::header::Header;
use ckb_core::transaction::OutPoint;
use ckb_core::transaction_meta::TransactionMeta;
use ckb_db::{Col, DBVector, Error, RocksDBTransaction, RocksDBTransactionSnapshot};
use ckb_protos::{self as protos, CanBuild};
use numext_fixed_hash::H256;

pub struct StoreTransaction {
    pub(crate) inner: RocksDBTransaction,
}

impl<'a> ChainStore<'a> for StoreTransaction {
    type Vector = DBVector;

    fn get(&self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.inner.get(col, key).expect("db operation should be ok")
    }
}

impl<'a> ChainStore<'a> for RocksDBTransactionSnapshot<'a> {
    type Vector = DBVector;

    fn get(&self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.get(col, key).expect("db operation should be ok")
    }
}

impl StoreTransaction {
    pub fn insert_raw(&self, col: Col, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.inner.put(col, key, value)
    }

    pub fn delete(&self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)
    }

    pub fn commit(&self) -> Result<(), Error> {
        self.inner.commit()
    }

    pub fn get_snapshot(&self) -> RocksDBTransactionSnapshot<'_> {
        self.inner.get_snapshot()
    }

    pub fn get_update_for_tip_hash(
        &self,
        snapshot: &RocksDBTransactionSnapshot<'_>,
    ) -> Option<H256> {
        self.inner
            .get_for_update(COLUMN_META, META_TIP_HEADER_KEY, snapshot)
            .expect("db operation should be ok")
            .map(|slice| H256::from_slice(&slice.as_ref()[..]).expect("db safe access"))
    }

    pub fn insert_tip_header(&self, h: &Header) -> Result<(), Error> {
        self.insert_raw(COLUMN_META, META_TIP_HEADER_KEY, h.hash().as_bytes())
    }

    pub fn insert_block(&self, block: &Block) -> Result<(), Error> {
        let hash = block.header().hash().as_bytes();
        {
            let builder = protos::StoredHeader::full_build(block.header());
            self.insert_raw(COLUMN_BLOCK_HEADER, hash, builder.as_slice())?;
        }
        {
            let builder = protos::StoredUncleBlocks::full_build(block.uncles());
            self.insert_raw(COLUMN_BLOCK_UNCLE, hash, builder.as_slice())?;
        }
        {
            let builder = protos::StoredProposalShortIds::full_build(block.proposals());
            self.insert_raw(COLUMN_BLOCK_PROPOSAL_IDS, hash, builder.as_slice())?;
        }
        {
            let builder = protos::StoredBlockBody::full_build(block.transactions());
            self.insert_raw(COLUMN_BLOCK_BODY, hash, builder.as_slice())?;
        }
        Ok(())
    }

    pub fn insert_block_ext(&self, block_hash: &H256, ext: &BlockExt) -> Result<(), Error> {
        let builder = protos::BlockExt::full_build(ext);
        self.insert_raw(COLUMN_BLOCK_EXT, block_hash.as_bytes(), builder.as_slice())
    }

    pub fn attach_block(&self, block: &Block) -> Result<(), Error> {
        let header = block.header();
        let hash = header.hash();
        for (index, tx) in block.transactions().iter().enumerate() {
            let tx_hash = tx.hash();
            {
                let info = TransactionInfo {
                    block_hash: hash.to_owned(),
                    block_number: header.number(),
                    block_epoch: header.epoch(),
                    index,
                };
                let builder = protos::StoredTransactionInfo::full_build(&info);
                self.insert_raw(
                    COLUMN_TRANSACTION_INFO,
                    tx_hash.as_bytes(),
                    builder.as_slice(),
                )?;
            }
        }

        let number = block.header().number().to_le_bytes();
        self.insert_raw(COLUMN_INDEX, &number, hash.as_bytes())?;
        for uncle in block.uncles() {
            self.insert_raw(COLUMN_UNCLES, &uncle.hash().as_bytes(), &[])?;
        }
        self.insert_raw(COLUMN_INDEX, hash.as_bytes(), &number)
    }

    pub fn detach_block(&self, block: &Block) -> Result<(), Error> {
        for tx in block.transactions() {
            let tx_hash = tx.hash();
            self.delete(COLUMN_TRANSACTION_INFO, tx_hash.as_bytes())?;
        }

        for uncle in block.uncles() {
            self.delete(COLUMN_UNCLES, &uncle.hash().as_bytes())?;
        }
        self.delete(COLUMN_INDEX, &block.header().number().to_le_bytes())?;
        self.delete(COLUMN_INDEX, block.header().hash().as_bytes())
    }

    pub fn insert_block_epoch_index(
        &self,
        block_hash: &H256,
        epoch_hash: &H256,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_BLOCK_EPOCH,
            block_hash.as_bytes(),
            epoch_hash.as_bytes(),
        )
    }

    pub fn insert_epoch_ext(&self, hash: &H256, epoch: &EpochExt) -> Result<(), Error> {
        let epoch_index = hash.as_bytes();
        let epoch_number = epoch.number().to_le_bytes();
        let builder = protos::StoredEpochExt::full_build(epoch);
        self.insert_raw(COLUMN_EPOCH, epoch_index, builder.as_slice())?;
        self.insert_raw(COLUMN_EPOCH, &epoch_number, epoch_index)
    }

    pub fn insert_current_epoch_ext(&self, epoch: &EpochExt) -> Result<(), Error> {
        let builder = protos::StoredEpochExt::full_build(epoch);
        self.insert_raw(COLUMN_META, META_CURRENT_EPOCH_KEY, builder.as_slice())
    }

    pub fn update_cell_set(&self, tx_hash: &H256, meta: &TransactionMeta) -> Result<(), Error> {
        let builder = protos::TransactionMeta::full_build(meta);
        self.insert_raw(COLUMN_CELL_SET, tx_hash.as_bytes(), builder.as_slice())
    }

    pub fn delete_cell_set(&self, tx_hash: &H256) -> Result<(), Error> {
        self.delete(COLUMN_CELL_SET, tx_hash.as_bytes())
    }
}

impl CellProvider for StoreTransaction {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        match self.get_tx_meta(&out_point.tx_hash) {
            Some(tx_meta) => match tx_meta.is_dead(out_point.index as usize) {
                Some(false) => {
                    let mut cell_meta = self
                        .get_cell_meta(&out_point.tx_hash, out_point.index)
                        .expect("store should be consistent with cell_set");
                    if with_data {
                        cell_meta.mem_cell_data =
                            self.get_cell_data(&out_point.tx_hash, out_point.index);
                    }
                    CellStatus::live_cell(cell_meta)
                }
                Some(true) => CellStatus::Dead,
                None => CellStatus::Unknown,
            },
            None => CellStatus::Unknown,
        }
    }
}

impl HeaderProvider for StoreTransaction {
    fn header(&self, block_hash: &H256, out_point: Option<&OutPoint>) -> HeaderStatus {
        match self.get_block_header(block_hash) {
            Some(header) => {
                if let Some(out_point) = out_point {
                    self.get_transaction_info(&out_point.tx_hash).map_or(
                        HeaderStatus::InclusionFaliure,
                        |info| {
                            if info.block_hash == *block_hash {
                                HeaderStatus::live_header(header)
                            } else {
                                HeaderStatus::InclusionFaliure
                            }
                        },
                    )
                } else {
                    HeaderStatus::live_header(header)
                }
            }
            None => HeaderStatus::Unknown,
        }
    }
}
