use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE,
    COLUMN_NUMBER_HASH,
};
use ckb_db::RocksDBWriteBatch;
use ckb_error::Error;
use ckb_types::{core::BlockNumber, packed, prelude::*};

pub struct StoreWriteBatch {
    pub(crate) inner: RocksDBWriteBatch,
}

impl StoreWriteBatch {
    pub fn delete_block_body(
        &mut self,
        number: BlockNumber,
        hash: &packed::Byte32,
        txs_len: u32,
    ) -> Result<(), Error> {
        self.inner.delete(COLUMN_BLOCK_UNCLE, hash.as_slice())?;
        self.inner
            .delete(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())?;
        self.inner.delete(
            COLUMN_NUMBER_HASH,
            packed::NumberHash::new_builder()
                .number(number.pack())
                .block_hash(hash.clone())
                .build()
                .as_slice(),
        )?;
        let txs_start_key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(0u32.pack())
            .build();

        let txs_end_key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(txs_len.pack())
            .build();

        self.inner.delete_range(
            COLUMN_BLOCK_BODY,
            txs_start_key.as_slice(),
            txs_end_key.as_slice(),
        )?;
        Ok(())
    }

    pub fn delete_block(
        &mut self,
        number: BlockNumber,
        hash: &packed::Byte32,
        txs_len: u32,
    ) -> Result<(), Error> {
        self.inner.delete(COLUMN_BLOCK_HEADER, hash.as_slice())?;
        self.delete_block_body(number, hash, txs_len)
    }

    pub fn clear(&mut self) -> Result<(), Error> {
        self.inner.clear()
    }
}
