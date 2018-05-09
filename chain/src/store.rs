use avl::node::search;
use avl::tree::AvlTree;
use bigint::H256;
use core::block::Block;
use core::header::Header;
use core::transaction::{OutPoint, Transaction, TransactionAddress};
use core::transaction_meta::TransactionMeta;
use db::batch::{Batch, Key, KeyValue, Value};
use db::kvdb::KeyValueDB;

const META_HEAD_HEADER_KEY: &str = "HEAD_HEADER";

pub trait ChainStore: Sync + Send {
    fn get_block(&self, h: &H256) -> Option<Block>;
    fn get_header(&self, h: &H256) -> Option<Header>;
    fn get_block_hash(&self, height: u64) -> Option<H256>;
    fn get_output_root(&self, h: H256) -> Option<H256>;
    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>>;
    fn get_transaction(&self, h: &H256) -> Option<Transaction>;
    fn get_transaction_meta(&self, root: H256, key: H256) -> Option<TransactionMeta>;
    fn update_transaction_meta(
        &self,
        root: H256,
        inputs: Vec<OutPoint>,
        outputs: Vec<OutPoint>,
    ) -> Option<H256>;
    fn save_block(&self, b: &Block);
    fn head_header(&self) -> Option<Header>;
    fn save_head_header(&self, h: &Header);
    fn save_output_root(&self, h: H256, r: H256);
    fn save_block_hash(&self, height: u64, hash: &H256);
    fn delete_block_hash(&self, height: u64);
    fn save_transaction_address(&self, hash: &H256, txs: &[Transaction]);
    fn delete_transaction_address(&self, txs: &[Transaction]);
    fn init(&self, genesis: &Block);

    /// Visits block headers backward to genesis.
    fn headers_iter<'a>(&'a self, head: Header) -> ChainStoreBlockIterator<'a, Self>
    where
        Self: 'a + Sized,
    {
        ChainStoreBlockIterator {
            store: self,
            head: Some(head),
        }
    }
}

pub struct ChainStoreBlockIterator<'a, T: ChainStore>
where
    T: 'a,
{
    store: &'a T,
    head: Option<Header>,
}

pub struct ChainKVStore<T: KeyValueDB> {
    pub db: T,
}

impl<'a, T: ChainStore> ChainStoreBlockIterator<'a, T> {
    pub fn peek(&self) -> Option<&Header> {
        self.head.as_ref()
    }
}

impl<'a, T: ChainStore> Iterator for ChainStoreBlockIterator<'a, T> {
    type Item = Header;

    fn next(&mut self) -> Option<Self::Item> {
        let current_header = self.head.take();
        self.head = match current_header {
            Some(ref h) => {
                if h.height > 0 {
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
            Some(ref h) => (1, Some(h.height as usize + 1)),
            None => (0, Some(0)),
        }
    }
}

impl<T: KeyValueDB> ChainKVStore<T> {
    fn get(&self, key: &Key) -> Option<Value> {
        self.db.read(key).expect("db operation should be ok")
    }

    fn put(&self, batch: Batch) {
        self.db.write(batch).expect("db operation should be ok")
    }

    fn save_block_with_batch(&self, batch: &mut Batch, b: &Block) {
        batch.insert(KeyValue::BlockHeader(b.hash(), Box::new(b.header.clone())));
        batch.insert(KeyValue::BlockBody(b.hash(), b.transactions.clone()));
    }

    fn save_head_header_with_batch(&self, batch: &mut Batch, h: &Header) {
        batch.insert(KeyValue::Meta(META_HEAD_HEADER_KEY, h.hash().to_vec()));
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

    fn save_block(&self, b: &Block) {
        let mut batch = Batch::default();
        self.save_block_with_batch(&mut batch, b);
        self.put(batch);
    }

    fn get_header(&self, h: &H256) -> Option<Header> {
        self.get(&Key::BlockHeader(*h)).and_then(|v| match v {
            Value::BlockHeader(h) => Some(*h),
            _ => None,
        })
    }

    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.get(&Key::BlockBody(*h)).and_then(|v| match v {
            Value::BlockBody(b) => Some(b),
            _ => None,
        })
    }

    fn get_transaction(&self, h: &H256) -> Option<Transaction> {
        self.get(&Key::TransactionAddress(*h))
            .and_then(|v| match v {
                Value::TransactionAddress(d) => self
                    .get_block_body(&d.hash)
                    .and_then(|v| Some(v[d.index as usize].clone())),
                _ => None,
            })
    }

    fn get_transaction_meta(&self, root: H256, key: H256) -> Option<TransactionMeta> {
        search(&self.db, root, key).expect("tree operation error")
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

    fn get_output_root(&self, h: H256) -> Option<H256> {
        self.get(&Key::OutputRoot(h)).and_then(|v| match v {
            Value::OutputRoot(r) => Some(r),
            _ => None,
        })
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
        self.save_head_header_with_batch(&mut batch, h);
        self.put(batch);
    }

    fn save_output_root(&self, h: H256, r: H256) {
        let mut batch = Batch::default();
        batch.insert(KeyValue::OutputRoot(h, r));
        self.put(batch);
    }

    fn save_block_hash(&self, height: u64, hash: &H256) {
        let mut batch = Batch::default();
        batch.insert(KeyValue::BlockHash(height, *hash));
        self.put(batch);
    }

    fn save_transaction_address(&self, hash: &H256, txs: &[Transaction]) {
        let mut batch = Batch::default();
        for (id, tx) in txs.iter().enumerate() {
            batch.insert(KeyValue::TransactionAddress(
                tx.hash(),
                TransactionAddress {
                    hash: *hash,
                    index: id as u32,
                },
            ));
        }
        self.put(batch);
    }

    fn delete_transaction_address(&self, txs: &[Transaction]) {
        let mut batch = Batch::default();
        for tx in txs {
            batch.delete(Key::TransactionAddress(tx.hash()));
        }
        self.put(batch);
    }

    fn delete_block_hash(&self, height: u64) {
        let mut batch = Batch::default();
        batch.delete(Key::BlockHash(height));
        self.put(batch);
    }

    fn init(&self, genesis: &Block) {
        self.save_block(genesis);
        self.save_head_header(&genesis.header);
        self.save_output_root(genesis.hash(), H256::zero());
        self.save_block_hash(0, &genesis.hash());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::block::Block;
    use core::header::Header;
    use db::memorydb::MemoryKeyValueDB;

    #[test]
    fn save_and_get_block() {
        let db = MemoryKeyValueDB::default();
        let store = ChainKVStore { db: db };
        let header = Header::default();

        let block = Block {
            header,
            transactions: vec![],
        };

        let hash = block.hash();
        store.save_block(&block);
        assert_eq!(block, store.get_block(&hash).unwrap());
    }
}
