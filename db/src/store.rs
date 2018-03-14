use batch::{Batch, Key, KeyValue, Value};
use bigint::H256;
use core::block::{Block, Header};
use core::transaction::Transaction;
use kvdb::KeyValueDB;

const META_HEAD_HEADER_KEY: &str = "HEAD_HEADER";

pub trait ChainStore: Sync + Send {
    fn get_block(&self, h: &H256) -> Option<Block>;
    fn get_header(&self, h: &H256) -> Option<Header>;
    fn get_block_hash(&self, height: u64) -> Option<H256>;
    fn get_block_transactions(&self, h: &H256) -> Option<Vec<Transaction>>;
    fn get_transaction(&self, h: &H256) -> Option<Transaction>;
    fn save_block(&self, b: &Block);
    fn save_block_hash(&self, height: u64, hash: &H256);
    fn head_header(&self) -> Option<Header>;
    fn save_head_header(&self, h: &Header);
    fn init(&self, genesis: &Block) -> ();
}

pub struct ChainKVStore<T: KeyValueDB> {
    pub db: Box<T>,
}

impl<T: KeyValueDB> ChainKVStore<T> {
    fn get(&self, key: &Key) -> Option<Value> {
        self.db.read(key).expect("db operation should be ok")
    }

    fn put(&self, batch: Batch) {
        self.db.write(batch).expect("db operation should be ok")
    }
}

impl<T: KeyValueDB> ChainStore for ChainKVStore<T> {
    // TODO error log
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.get_header(h).and_then(|header| {
            let transactions = self.get_block_transactions(h).unwrap();
            Some(Block {
                header,
                transactions,
            })
        })
    }

    fn save_block(&self, b: &Block) {
        let mut batch = Batch::default();
        batch.insert(KeyValue::BlockHeader(b.hash(), Box::new(b.header.clone())));
        batch.insert(KeyValue::BlockTransactions(
            b.hash(),
            b.transactions.iter().map(|tx| tx.hash()).collect(),
        ));
        self.put(batch);
    }

    fn get_header(&self, h: &H256) -> Option<Header> {
        self.get(&Key::BlockHeader(*h)).and_then(|v| match v {
            Value::BlockHeader(h) => Some(*h),
            _ => None,
        })
    }

    fn get_block_transactions(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.get(&Key::BlockTransactions(*h)).and_then(|v| match v {
            Value::BlockTransactions(hashes) => {
                hashes.iter().map(|h| self.get_transaction(h)).collect()
            }
            _ => None,
        })
    }

    fn get_transaction(&self, h: &H256) -> Option<Transaction> {
        self.get(&Key::Transaction(*h)).and_then(|v| match v {
            Value::Transaction(t) => Some(*t),
            _ => None,
        })
    }

    fn save_block_hash(&self, height: u64, hash: &H256) {
        let mut batch = Batch::default();
        batch.insert(KeyValue::BlockHash(height, *hash));
        self.put(batch);
    }

    fn get_block_hash(&self, height: u64) -> Option<H256> {
        self.get(&Key::BlockHash(height)).and_then(|v| match v {
            Value::BlockHash(h) => Some(h),
            _ => None,
        })
    }

    fn head_header(&self) -> Option<Header> {
        self.get(&Key::Meta(META_HEAD_HEADER_KEY))
            .and_then(|v| match v {
                Value::Meta(data) => self.get_header(&H256::from(&data[..])),
                _ => None,
            })
    }

    fn save_head_header(&self, h: &Header) {
        let mut batch = Batch::default();
        batch.insert(KeyValue::Meta(META_HEAD_HEADER_KEY, h.hash().to_vec()));
        self.put(batch);
    }

    fn init(&self, genesis: &Block) {
        self.save_block(genesis);
        self.save_head_header(&genesis.header);
        self.save_block_hash(genesis.header.height, &genesis.hash());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::{H256, H520, U256};
    use core::block::{Block, Header, RawHeader};
    use core::proof::Proof;
    use memorydb::MemoryKeyValueDB;

    #[test]
    fn save_and_get_block() {
        let db = MemoryKeyValueDB::default();
        let store = ChainKVStore { db: Box::new(db) };
        let raw_header = RawHeader {
            pre_hash: H256::from(0),
            timestamp: 0,
            transactions_root: H256::from(0),
            difficulty: U256::from(0),
            challenge: H256::from(0),
            proof: Proof::default(),
            height: 0,
        };

        let block = Block {
            header: Header::new(raw_header, U256::from(0), Some(H520::from(0))),
            transactions: vec![],
        };

        let hash = block.hash();
        store.save_block(&block);
        assert_eq!(block, store.get_block(&hash).unwrap());
    }
}
