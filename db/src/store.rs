use batch::{Batch, Key, KeyValue, Value};
use bigint::H256;
use core::block::{Block, Header};
use core::chain::HeadRoute;
use core::transaction::Transaction;
use kvdb::KeyValueDB;
use std::collections::HashMap;
use transaction_meta::TransactionMeta;

const META_HEAD_HEADER_KEY: &str = "HEAD_HEADER";

pub trait ChainStore: Sync + Send {
    fn get_block(&self, h: &H256) -> Option<Block>;
    fn get_header(&self, h: &H256) -> Option<Header>;
    fn get_block_hash(&self, height: u64) -> Option<H256>;
    fn get_block_height(&self, h: &H256) -> Option<u64>;
    fn get_block_transactions(&self, h: &H256) -> Option<Vec<Transaction>>;
    fn get_transaction(&self, h: &H256) -> Option<Transaction>;
    fn get_transaction_meta(&self, h: &H256) -> Option<TransactionMeta>;
    fn save_block(&self, b: &Block);
    fn head_header(&self) -> Option<Header>;
    fn save_head_header(&self, h: &Header) -> HeadRoute;
    fn init(&self, genesis: &Block) -> ();

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
                    self.store.get_header(&h.pre_hash)
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

        let txids = b.transactions
            .iter()
            .map(|tx| tx.hash())
            .collect::<Vec<H256>>();
        // TODO: do not save known transactions
        for (tx, txid) in b.transactions.iter().zip(txids.iter()) {
            batch.insert(KeyValue::Transaction(*txid, Box::new(tx.clone())));
        }
        batch.insert(KeyValue::BlockTransactions(b.hash(), txids));
    }

    fn get_transaction_meta_mut_with_meta_table<'a>(
        &self,
        meta_table: &'a mut HashMap<H256, Option<TransactionMeta>>,
        hash: &H256,
    ) -> &'a mut Option<TransactionMeta> {
        meta_table
            .entry(*hash)
            .or_insert_with(|| self.get_transaction_meta(hash))
    }

    fn append_block_with_meta_table(
        &self,
        meta_table: &mut HashMap<H256, Option<TransactionMeta>>,
        hash: &H256,
    ) {
        for tx in self.get_block_transactions(hash)
            .expect("block transactions must be stored")
        {
            {
                let mut meta_option =
                    self.get_transaction_meta_mut_with_meta_table(meta_table, hash);

                *meta_option = Some(match meta_option.take() {
                    Some(mut meta) => {
                        assert!(meta.is_fully_spent(), "Tx conflict, not fully spent yet");
                        assert!(
                            meta.len() >= tx.outputs.len(),
                            "Tx conflict, hash collision"
                        );
                        meta.renew();
                        meta
                    }
                    None => TransactionMeta::new(0, tx.outputs.len()),
                });
            }

            for input in &tx.inputs {
                let meta = self.get_transaction_meta_mut_with_meta_table(
                    meta_table,
                    &input.previous_output.hash,
                ).as_mut()
                    .expect("block transaction input meta must be stored");

                meta.set_spent(input.previous_output.index as usize);
            }
        }
    }

    fn rollback_block_with_meta_table(
        &self,
        meta_table: &mut HashMap<H256, Option<TransactionMeta>>,
        hash: &H256,
    ) {
        for tx in self.get_block_transactions(hash)
            .expect("block transactions must be stored")
            .iter()
            .rev()
        {
            {
                let mut meta_option =
                    self.get_transaction_meta_mut_with_meta_table(meta_table, hash);

                let mut meta = meta_option.take().expect("Tx meta not found when rollback");

                assert!(meta.is_new(), "Tx conflict, cannot rollback");
                assert!(
                    meta.len() >= tx.outputs.len(),
                    "Tx conflict, hash collision"
                );

                if meta.fully_spent_count > 0 {
                    meta.rollback();
                    *meta_option = Some(meta);
                }
            }

            for input in &tx.inputs {
                match *self.get_transaction_meta_mut_with_meta_table(
                    meta_table,
                    &input.previous_output.hash,
                ) {
                    Some(ref mut meta) => meta.unset_spent(input.previous_output.index as usize),
                    _ => {
                        unreachable!("Tx meta not found when rollback");
                    }
                }
            }
        }
    }

    fn save_head_header_with_batch(&self, batch: &mut Batch, h: &Header) -> HeadRoute {
        let mut route = HeadRoute::new(h.hash());
        let mut append_reversed = Vec::<H256>::new();

        match self.head_header() {
            Some(old_head) => {
                let mut old_head_height = old_head.height;
                let mut old_head_iter = self.headers_iter(old_head);

                while old_head_height > h.height {
                    let current_old_head = old_head_iter.next().unwrap();
                    batch.delete(Key::BlockHash(current_old_head.height));
                    batch.delete(Key::BlockHeight(current_old_head.hash()));
                    route.rollback.push(current_old_head.hash());
                    old_head_height -= 1;
                }

                let mut new_head_iter = self.headers_iter(h.clone());

                for _ in old_head_height..h.height {
                    let current_new_head = new_head_iter.next().unwrap();
                    let hash = current_new_head.hash();
                    batch.insert(KeyValue::BlockHash(current_new_head.height, hash));
                    batch.insert(KeyValue::BlockHeight(hash, current_new_head.height));
                    append_reversed.push(hash);
                }

                for (current_old_head, current_new_head) in old_head_iter.zip(new_head_iter) {
                    let old_hash = current_old_head.hash();
                    let new_hash = current_new_head.hash();
                    if old_hash == new_hash {
                        break;
                    }

                    batch.insert(KeyValue::BlockHash(current_new_head.height, new_hash));
                    batch.insert(KeyValue::BlockHeight(new_hash, current_new_head.height));
                    route.rollback.push(old_hash);
                    append_reversed.push(new_hash);
                }
            }
            None => for header in self.headers_iter(h.clone()) {
                let hash = header.hash();
                batch.insert(KeyValue::BlockHash(header.height, hash));
                batch.insert(KeyValue::BlockHeight(hash, header.height));
                append_reversed.push(hash);
            },
        }

        append_reversed.reverse();
        route.append = append_reversed;

        let mut meta_table = HashMap::<H256, Option<TransactionMeta>>::new();
        for hash in &route.rollback {
            self.rollback_block_with_meta_table(&mut meta_table, hash);
        }
        for hash in &route.append {
            self.append_block_with_meta_table(&mut meta_table, hash);
        }
        for (hash, meta) in meta_table {
            match meta {
                Some(m) => batch.insert(KeyValue::TransactionMeta(hash, Box::new(m))),
                None => batch.delete(Key::TransactionMeta(hash)),
            }
        }

        batch.insert(KeyValue::Meta(META_HEAD_HEADER_KEY, h.hash().to_vec()));

        route
    }
}

impl<T: KeyValueDB> ChainStore for ChainKVStore<T> {
    // TODO error log
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.get_header(h).and_then(|header| {
            let transactions = self.get_block_transactions(h)
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

    fn get_transaction_meta(&self, h: &H256) -> Option<TransactionMeta> {
        self.get(&Key::TransactionMeta(*h)).and_then(|v| match v {
            Value::TransactionMeta(t) => Some(*t),
            _ => None,
        })
    }

    fn get_block_hash(&self, height: u64) -> Option<H256> {
        self.get(&Key::BlockHash(height)).and_then(|v| match v {
            Value::BlockHash(h) => Some(h),
            _ => None,
        })
    }

    fn get_block_height(&self, hash: &H256) -> Option<u64> {
        self.get(&Key::BlockHeight(*hash)).and_then(|v| match v {
            Value::BlockHeight(h) => Some(h),
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

    fn save_head_header(&self, h: &Header) -> HeadRoute {
        let mut batch = Batch::default();
        let route = self.save_head_header_with_batch(&mut batch, h);
        self.put(batch);
        route
    }

    fn init(&self, genesis: &Block) {
        self.save_block(genesis);
        self.save_head_header(&genesis.header);
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
        let store = ChainKVStore { db: db };
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
