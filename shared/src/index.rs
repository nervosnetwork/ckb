use crate::flat_serializer::serialized_addresses;
use crate::store::{ChainKVStore, ChainStore, DefaultStoreBatch, StoreBatch};
use crate::{COLUMN_BLOCK_BODY, COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_ADDR};
use bincode::{deserialize, serialize};
use ckb_core::block::Block;
use ckb_core::extras::{BlockExt, TransactionAddress};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Transaction, TransactionBuilder};
use ckb_db::batch::Batch;
use ckb_db::kvdb::{DbBatch, KeyValueDB};
use numext_fixed_hash::H256;

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";

// maintain chain index, extend chainstore
pub trait ChainIndex: ChainStore {
    fn init(&self, genesis: &Block);
    fn get_block_hash(&self, number: BlockNumber) -> Option<H256>;
    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber>;
    fn get_tip_header(&self) -> Option<Header>;
    fn get_transaction(&self, h: &H256) -> Option<Transaction>;
    fn get_transaction_address(&self, hash: &H256) -> Option<TransactionAddress>;
}

impl<T: KeyValueDB> ChainIndex for ChainKVStore<T> {
    fn init(&self, genesis: &Block) {
        let mut batch = self.new_batch();
        let genesis_hash = genesis.header().hash();
        let ext = BlockExt {
            received_at: genesis.header().timestamp(),
            total_difficulty: genesis.header().difficulty().clone(),
            total_uncles_count: 0,
            txs_verified: Some(true),
        };

        let mut cells = Vec::with_capacity(genesis.commit_transactions().len());

        for tx in genesis.commit_transactions() {
            let ins = if tx.is_cellbase() {
                Vec::new()
            } else {
                tx.input_pts()
            };
            let outs = tx.output_pts();

            cells.push((ins, outs));
        }

        batch.insert_block(genesis);
        batch.insert_block_ext(&genesis_hash, &ext);
        batch.insert_tip_header(&genesis.header());
        batch.insert_block_hash(0, &genesis_hash);
        batch.insert_block_number(&genesis_hash, 0);
        batch.insert_transaction_address(&genesis_hash, genesis.commit_transactions());
        batch.commit();
    }

    fn get_block_hash(&self, number: BlockNumber) -> Option<H256> {
        let key = serialize(&number).unwrap();
        self.get(COLUMN_INDEX, &key)
            .map(|raw| H256::from_slice(&raw[..]).expect("db safe access"))
    }

    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.get(COLUMN_INDEX, hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_tip_header(&self) -> Option<Header> {
        self.get(COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|raw| self.get_header(&H256::from_slice(&raw[..]).expect("db safe access")))
            .map(Into::into)
    }

    fn get_transaction(&self, h: &H256) -> Option<Transaction> {
        self.get_transaction_address(h)
            .and_then(|d| {
                self.partial_get(
                    COLUMN_BLOCK_BODY,
                    d.block_hash.as_bytes(),
                    &(d.offset..(d.offset + d.length)),
                )
            })
            .map(|ref serialized_transaction| {
                TransactionBuilder::new(serialized_transaction).build()
            })
    }

    fn get_transaction_address(&self, h: &H256) -> Option<TransactionAddress> {
        self.get(COLUMN_TRANSACTION_ADDR, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_db::{DBConfig, RocksDB};
    use tempfile;

    #[test]
    fn index_store() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("index_init")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        let db = RocksDB::open(&config, COLUMNS);
        let store = ChainKVStore::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();
        let hash = block.header().hash();
        store.init(&block);
        assert_eq!(&hash, &store.get_block_hash(0).unwrap());

        assert_eq!(
            block.header().difficulty(),
            &store.get_block_ext(&hash).unwrap().total_difficulty
        );

        assert_eq!(
            block.header().number(),
            store.get_block_number(&hash).unwrap()
        );

        assert_eq!(block.header(), &store.get_tip_header().unwrap());
    }
}
