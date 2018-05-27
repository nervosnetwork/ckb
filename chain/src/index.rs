use bigint::H256;
use bincode::{deserialize, serialize};
use core::block::Block;
use core::extras::{BlockExt, TransactionAddress};
use core::header::Header;
use core::transaction::Transaction;
use db::batch::Batch;
use db::kvdb::KeyValueDB;
use store::{ChainKVStore, ChainStore};
use {COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_ADDR};

const META_HEAD_HEADER_KEY: &[u8] = b"HEAD_HEADER";

// maintain chain index, extend chainstore
pub trait ChainIndex: ChainStore {
    fn init(&self, genesis: &Block);
    fn get_block_hash(&self, height: u64) -> Option<H256>;
    fn get_block_height(&self, hash: &H256) -> Option<u64>;
    fn get_head_header(&self) -> Option<Header>;
    fn get_transaction(&self, h: &H256) -> Option<Transaction>;
    fn get_transaction_address(&self, hash: &H256) -> Option<TransactionAddress>;

    fn insert_block_hash(&self, batch: &mut Batch, height: u64, hash: &H256);
    fn delete_block_hash(&self, batch: &mut Batch, height: u64);
    fn insert_block_height(&self, batch: &mut Batch, hash: &H256, height: u64);
    fn delete_block_height(&self, batch: &mut Batch, hash: &H256);
    fn insert_head_header(&self, batch: &mut Batch, h: &Header);
    fn insert_transaction_address(&self, batch: &mut Batch, block_hash: &H256, txs: &[Transaction]);
    fn delete_transaction_address(&self, batch: &mut Batch, txs: &[Transaction]);
}

impl<T: KeyValueDB> ChainIndex for ChainKVStore<T> {
    fn init(&self, genesis: &Block) {
        self.save_with_batch(|batch| {
            let ext = BlockExt {
                received_at: genesis.header.timestamp,
                total_difficulty: genesis.header.difficulty,
            };
            self.insert_block(batch, genesis);
            self.insert_block_ext(batch, &genesis.hash(), &ext);
            self.insert_head_header(batch, &genesis.header);
            self.insert_output_root(batch, genesis.hash(), H256::zero());
            self.insert_block_hash(batch, 0, &genesis.hash());
            self.insert_block_height(batch, &genesis.hash(), 0);
        });
    }

    fn get_block_hash(&self, height: u64) -> Option<H256> {
        let key = serialize(&height).unwrap();
        self.get(COLUMN_INDEX, &key).map(|raw| H256::from(&raw[..]))
    }

    fn get_block_height(&self, hash: &H256) -> Option<u64> {
        self.get(COLUMN_INDEX, &hash)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_head_header(&self) -> Option<Header> {
        self.get(COLUMN_META, META_HEAD_HEADER_KEY)
            .and_then(|raw| self.get_header(&H256::from(&raw[..])))
    }

    fn get_transaction(&self, h: &H256) -> Option<Transaction> {
        self.get_transaction_address(h).and_then(|d| {
            self.get_block_body(&d.block_hash)
                .map(|txs| txs[d.index as usize].clone())
        })
    }

    fn get_transaction_address(&self, h: &H256) -> Option<TransactionAddress> {
        self.get(COLUMN_TRANSACTION_ADDR, &h)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn insert_head_header(&self, batch: &mut Batch, h: &Header) {
        batch.insert(
            COLUMN_META,
            META_HEAD_HEADER_KEY.to_vec(),
            h.hash().to_vec(),
        );
    }

    fn insert_block_hash(&self, batch: &mut Batch, height: u64, hash: &H256) {
        let key = serialize(&height).unwrap().to_vec();
        batch.insert(COLUMN_INDEX, key, hash.to_vec());
    }

    fn insert_block_height(&self, batch: &mut Batch, hash: &H256, height: u64) {
        batch.insert(
            COLUMN_INDEX,
            hash.to_vec(),
            serialize(&height).unwrap().to_vec(),
        );
    }

    fn insert_transaction_address(
        &self,
        batch: &mut Batch,
        block_hash: &H256,
        txs: &[Transaction],
    ) {
        for (id, tx) in txs.iter().enumerate() {
            let address = TransactionAddress {
                block_hash: *block_hash,
                index: id as u32,
            };
            batch.insert(
                COLUMN_TRANSACTION_ADDR,
                tx.hash().to_vec(),
                serialize(&address).unwrap().to_vec(),
            );
        }
    }

    fn delete_transaction_address(&self, batch: &mut Batch, txs: &[Transaction]) {
        for tx in txs {
            batch.delete(COLUMN_TRANSACTION_ADDR, tx.hash().to_vec());
        }
    }

    fn delete_block_hash(&self, batch: &mut Batch, height: u64) {
        let key = serialize(&height).unwrap().to_vec();
        batch.delete(COLUMN_INDEX, key);
    }

    fn delete_block_height(&self, batch: &mut Batch, hash: &H256) {
        batch.delete(COLUMN_INDEX, hash.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Spec, COLUMNS};
    use super::*;
    use db::diskdb::RocksDB;
    use tempdir::TempDir;

    #[test]
    fn index_store() {
        let tmp_dir = TempDir::new("index_init").unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore { db: db };
        let block = Spec::default().genesis_block();
        let hash = block.hash();
        store.init(&block);
        assert_eq!(hash, store.get_block_hash(0).unwrap());

        assert_eq!(
            block.header.difficulty,
            store.get_block_ext(&hash).unwrap().total_difficulty
        );

        assert_eq!(block.header.height, store.get_block_height(&hash).unwrap());

        assert_eq!(block.header, store.get_head_header().unwrap());
    }
}
