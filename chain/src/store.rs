use avl::node::search;
use avl::tree::AvlTree;
use bigint::H256;
use bincode::{deserialize, serialize};
use core::block::Block;
use core::extras::BlockExt;
use core::header::Header;
use core::transaction::{OutPoint, Transaction};
use core::transaction_meta::TransactionMeta;
use db::batch::{Batch, Col};
use db::kvdb::KeyValueDB;
use {COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_EXT, COLUMN_OUTPUT_ROOT};

pub struct ChainKVStore<T: KeyValueDB> {
    pub db: T,
}

impl<T: KeyValueDB> ChainKVStore<T> {
    pub fn get(&self, col: Col, key: &[u8]) -> Option<Vec<u8>> {
        self.db.read(col, key).expect("db operation should be ok")
    }
}

pub struct ChainStoreHeaderIterator<'a, T: ChainStore>
where
    T: 'a,
{
    store: &'a T,
    head: Option<Header>,
}

pub trait ChainStore: Sync + Send {
    fn get_block(&self, block_hash: &H256) -> Option<Block>;
    fn get_header(&self, block_hash: &H256) -> Option<Header>;
    fn get_output_root(&self, block_hash: &H256) -> Option<H256>;
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    fn get_transaction_meta(&self, root: H256, key: H256) -> Option<TransactionMeta>;
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;

    fn update_transaction_meta(
        &self,
        root: H256,
        inputs: Vec<OutPoint>,
        outputs: Vec<OutPoint>,
    ) -> Option<H256>;

    fn insert_block(&self, batch: &mut Batch, b: &Block);
    fn insert_block_ext(&self, batch: &mut Batch, block_hash: &H256, ext: &BlockExt);
    fn insert_output_root(&self, batch: &mut Batch, block_hash: H256, r: H256);
    fn save_with_batch<F: FnOnce(&mut Batch)>(&self, f: F);

    /// Visits block headers backward to genesis.
    fn headers_iter<'a>(&'a self, head: Header) -> ChainStoreHeaderIterator<'a, Self>
    where
        Self: 'a + Sized,
    {
        ChainStoreHeaderIterator {
            store: self,
            head: Some(head),
        }
    }
}

impl<'a, T: ChainStore> Iterator for ChainStoreHeaderIterator<'a, T> {
    type Item = Header;

    fn next(&mut self) -> Option<Self::Item> {
        let current_header = self.head.take();
        self.head = match current_header {
            Some(ref h) => {
                if h.number > 0 {
                    self.store.get_header(&h.parent_hash)
                } else {
                    None
                }
            }
            None => None,
        };
        current_header
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self.head {
            Some(ref h) => (1, Some(h.number as usize + 1)),
            None => (0, Some(0)),
        }
    }
}

impl<T: KeyValueDB> ChainStore for ChainKVStore<T> {
    // TODO error log
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.get_header(h).and_then(|header| {
            let transactions = self
                .get_block_body(h)
                .expect("block transactions must be stored");
            Some(Block {
                header,
                transactions,
            })
        })
    }

    fn get_header(&self, h: &H256) -> Option<Header> {
        self.get(COLUMN_BLOCK_HEADER, &h)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.get(COLUMN_BLOCK_BODY, &h)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.get(COLUMN_EXT, &block_hash)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_transaction_meta(&self, root: H256, key: H256) -> Option<TransactionMeta> {
        search(&self.db, root, key).expect("tree operation error")
    }

    fn get_output_root(&self, block_hash: &H256) -> Option<H256> {
        self.get(COLUMN_OUTPUT_ROOT, block_hash)
            .map(|raw| H256::from(&raw[..]))
    }

    fn update_transaction_meta(
        &self,
        root: H256,
        inputs: Vec<OutPoint>,
        outputs: Vec<OutPoint>,
    ) -> Option<H256> {
        let mut avl = AvlTree::new(&self.db, root);

        for input in inputs {
            if !avl
                .update(input.hash, input.index as usize)
                .expect("tree operation error")
            {
                return None;
            }
        }

        let len = outputs.len();

        if len != 0 {
            let hash = outputs[0].hash;
            let meta = TransactionMeta::new(0, len);
            match avl.insert(hash, meta).expect("tree operation error") {
                None => Some(avl.commit()),
                Some(mut old) => {
                    if old.is_fully_spent() {
                        old.renew();
                        avl.insert(hash, old).expect("tree operation error"); //Do we need the fully_spent_count?
                        Some(avl.commit())
                    } else {
                        None
                    }
                }
            }
        } else {
            Some(avl.commit())
        }
    }

    fn save_with_batch<F: FnOnce(&mut Batch)>(&self, f: F) {
        let mut batch = Batch::new();
        f(&mut batch);
        self.db.write(batch).expect("db operation should be ok")
    }

    fn insert_block(&self, batch: &mut Batch, b: &Block) {
        let hash = b.hash().to_vec();
        batch.insert(
            COLUMN_BLOCK_HEADER,
            hash.clone(),
            serialize(&b.header).unwrap().to_vec(),
        );
        batch.insert(
            COLUMN_BLOCK_BODY,
            hash,
            serialize(&b.transactions).unwrap().to_vec(),
        );
    }

    fn insert_block_ext(&self, batch: &mut Batch, block_hash: &H256, ext: &BlockExt) {
        batch.insert(
            COLUMN_EXT,
            block_hash.to_vec(),
            serialize(&ext).unwrap().to_vec(),
        );
    }

    fn insert_output_root(&self, batch: &mut Batch, block_hash: H256, r: H256) {
        batch.insert(COLUMN_OUTPUT_ROOT, block_hash.to_vec(), r.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Config, COLUMNS};
    use super::*;
    use db::diskdb::RocksDB;
    use tempdir::TempDir;

    #[test]
    fn save_and_get_output_root() {
        let tmp_dir = TempDir::new("save_and_get_output_root").unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore { db: db };

        store.save_with_batch(|batch| {
            store.insert_output_root(batch, H256::from(10), H256::from(20));
        });
        assert_eq!(
            H256::from(20),
            store.get_output_root(&H256::from(10)).unwrap()
        );
    }

    #[test]
    fn save_and_get_block() {
        let tmp_dir = TempDir::new("save_and_get_block").unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore { db: db };
        let block = Config::default().genesis_block();

        let hash = block.hash();

        store.save_with_batch(|batch| {
            store.insert_block(batch, &block);
        });
        assert_eq!(block, store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_ext() {
        let tmp_dir = TempDir::new("save_and_get_block_ext").unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore { db: db };
        let block = Config::default().genesis_block();

        let ext = BlockExt {
            received_at: block.header.timestamp,
            total_difficulty: block.header.difficulty,
        };

        let hash = block.hash();

        store.save_with_batch(|batch| {
            store.insert_block_ext(batch, &hash, &ext);
        });
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }
}
