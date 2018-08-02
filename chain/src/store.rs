use super::flat_serializer::{
    deserialize as flat_deserialize, serialize as flat_serialize, Address,
};
use avl::node::search;
use avl::tree::AvlTree;
use bigint::H256;
use bincode::{deserialize, serialize};
use core::block::IndexedBlock;
use core::extras::BlockExt;
use core::header::IndexedHeader;
use core::transaction::{OutPoint, Transaction};
use core::transaction_meta::TransactionMeta;
use core::uncle::UncleBlock;
use db::batch::{Batch, Col};
use db::kvdb::KeyValueDB;
use std::ops::Deref;
use std::ops::Range;
use {
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_TRANSACTION_ADDRESSES, COLUMN_BLOCK_UNCLE,
    COLUMN_EXT, COLUMN_OUTPUT_ROOT,
};

pub struct ChainKVStore<T: KeyValueDB> {
    pub db: T,
}

impl<T: KeyValueDB> ChainKVStore<T> {
    pub fn get(&self, col: Col, key: &[u8]) -> Option<Vec<u8>> {
        self.db.read(col, key).expect("db operation should be ok")
    }

    pub fn partial_get(&self, col: Col, key: &[u8], range: &Range<usize>) -> Option<Vec<u8>> {
        self.db
            .partial_read(col, key, range)
            .expect("db operation should be ok")
    }
}

pub struct ChainStoreHeaderIterator<'a, T: ChainStore>
where
    T: 'a,
{
    store: &'a T,
    head: Option<IndexedHeader>,
}

pub trait ChainStore: Sync + Send {
    fn get_block(&self, block_hash: &H256) -> Option<IndexedBlock>;
    fn get_header(&self, block_hash: &H256) -> Option<IndexedHeader>;
    fn get_output_root(&self, block_hash: &H256) -> Option<H256>;
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    fn get_block_uncles(&self, block_hash: &H256) -> Option<Vec<UncleBlock>>;
    fn get_transaction_meta(&self, root: H256, key: H256) -> Option<TransactionMeta>;
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;

    fn update_transaction_meta(
        &self,
        root: H256,
        cells: Vec<(Vec<OutPoint>, Vec<OutPoint>)>,
    ) -> Option<H256>;

    fn insert_block(&self, batch: &mut Batch, b: &IndexedBlock);
    fn insert_block_ext(&self, batch: &mut Batch, block_hash: &H256, ext: &BlockExt);
    fn insert_output_root(&self, batch: &mut Batch, block_hash: H256, r: H256);
    fn save_with_batch<F: FnOnce(&mut Batch)>(&self, f: F);

    /// Visits block headers backward to genesis.
    fn headers_iter<'a>(&'a self, head: IndexedHeader) -> ChainStoreHeaderIterator<'a, Self>
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
    type Item = IndexedHeader;

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
    fn get_block(&self, h: &H256) -> Option<IndexedBlock> {
        self.get_header(h).and_then(|header| {
            let transactions = self
                .get_block_body(h)
                .expect("block transactions must be stored");
            let uncles = self
                .get_block_uncles(h)
                .expect("block uncles must be stored");
            Some(IndexedBlock {
                header,
                transactions,
                uncles,
            })
        })
    }

    fn get_header(&self, h: &H256) -> Option<IndexedHeader> {
        self.get(COLUMN_BLOCK_HEADER, &h)
            .map(|raw| IndexedHeader::new(deserialize(&raw[..]).unwrap(), *h))
    }

    fn get_block_uncles(&self, h: &H256) -> Option<Vec<UncleBlock>> {
        self.get(COLUMN_BLOCK_UNCLE, &h)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.get(COLUMN_BLOCK_TRANSACTION_ADDRESSES, &h)
            .and_then(|serialized_addresses| {
                let addresses: Vec<Address> = deserialize(&serialized_addresses).unwrap();
                self.get(COLUMN_BLOCK_BODY, &h)
                    .map(|serialized_body| flat_deserialize(&serialized_body, &addresses).unwrap())
            })
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
        cells: Vec<(Vec<OutPoint>, Vec<OutPoint>)>,
    ) -> Option<H256> {
        let mut avl = AvlTree::new(&self.db, root);

        for (inputs, outputs) in cells {
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
                let meta = TransactionMeta::new(len);
                match avl.insert(hash, meta).expect("tree operation error") {
                    None => {}
                    Some(_) => {
                        // txid must be unique in chain
                        return None;
                    }
                }
            }
        }

        Some(avl.commit())
    }

    fn save_with_batch<F: FnOnce(&mut Batch)>(&self, f: F) {
        let mut batch = Batch::new();
        f(&mut batch);
        self.db.write(batch).expect("db operation should be ok")
    }

    fn insert_block(&self, batch: &mut Batch, b: &IndexedBlock) {
        let hash = b.hash().to_vec();
        batch.insert(
            COLUMN_BLOCK_HEADER,
            hash.clone(),
            serialize(&b.header.deref()).unwrap().to_vec(),
        );
        let (block_data, block_addresses) = flat_serialize(&b.transactions).unwrap();
        batch.insert(
            COLUMN_BLOCK_UNCLE,
            hash.clone(),
            serialize(&b.uncles).unwrap().to_vec(),
        );
        batch.insert(COLUMN_BLOCK_BODY, hash.clone(), block_data);
        batch.insert(
            COLUMN_BLOCK_TRANSACTION_ADDRESSES,
            hash,
            serialize(&block_addresses).unwrap().to_vec(),
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
    use super::super::COLUMNS;
    use super::*;
    use consensus::Consensus;
    use db::diskdb::RocksDB;
    use rand;
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
        let consensus = Consensus::default();
        let block = consensus.genesis_block();

        let hash = block.hash();

        store.save_with_batch(|batch| {
            store.insert_block(batch, &block);
        });
        assert_eq!(block, &store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_with_transactions() {
        let tmp_dir = TempDir::new("save_and_get_block_with_transaction").unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore { db: db };
        let consensus = Consensus::default();
        let mut block = consensus.genesis_block().clone();
        block.transactions.push(create_dummy_transaction());
        block.transactions.push(create_dummy_transaction());
        block.transactions.push(create_dummy_transaction());

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
        let consensus = Consensus::default();
        let block = consensus.genesis_block();

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

    fn create_dummy_transaction() -> Transaction {
        Transaction::new(rand::random(), Vec::new(), Vec::new(), Vec::new())
    }
}
