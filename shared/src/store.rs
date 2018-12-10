use crate::error::SharedError;
use crate::flat_serializer::{serialize as flat_serialize, Address};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_TRANSACTION_ADDRESSES, COLUMN_BLOCK_TRANSACTION_IDS, COLUMN_BLOCK_UNCLE,
    COLUMN_EXT, COLUMN_OUTPUT_ROOT, COLUMN_TRANSACTION_META,
};
use avl::node::search;
use avl::tree::AvlTree;
use bincode::{deserialize, serialize};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::extras::BlockExt;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction, TransactionBuilder};
use ckb_core::transaction_meta::TransactionMeta;
use ckb_core::uncle::UncleBlock;
use ckb_db::batch::{Batch, Col};
use ckb_db::kvdb::KeyValueDB;
use ckb_util::RwLock;
use numext_fixed_hash::H256;
use std::ops::Range;
use std::sync::Arc;

pub struct ChainKVStore<T: KeyValueDB> {
    pub db: Arc<T>,
    tree: RwLock<AvlTree>,
}

impl<T: 'static + KeyValueDB> ChainKVStore<T> {
    pub fn new(db: T) -> Self {
        let db = Arc::new(db);
        let tree = RwLock::new(AvlTree::new(
            Arc::<T>::clone(&db),
            COLUMN_TRANSACTION_META,
            H256::zero(),
        ));

        ChainKVStore { db, tree }
    }

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
    head: Option<Header>,
}

pub trait ChainStore: Sync + Send {
    fn get_block(&self, block_hash: &H256) -> Option<Block>;
    fn get_header(&self, block_hash: &H256) -> Option<Header>;
    fn get_output_root(&self, block_hash: &H256) -> Option<H256>;
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>>;
    fn get_block_uncles(&self, block_hash: &H256) -> Option<Vec<UncleBlock>>;
    fn get_transaction_meta(&self, root: &H256, key: &H256) -> Option<TransactionMeta>;
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;

    fn update_transaction_meta(
        &self,
        batch: &mut Batch,
        root: H256,
        cells: Vec<(Vec<OutPoint>, Vec<OutPoint>)>,
    ) -> Option<H256>;

    fn insert_block(&self, batch: &mut Batch, b: &Block);
    fn insert_block_ext(&self, batch: &mut Batch, block_hash: &H256, ext: &BlockExt);
    fn insert_output_root(&self, batch: &mut Batch, block_hash: &H256, r: &H256);
    fn save_with_batch<F: FnOnce(&mut Batch) -> Result<(), SharedError>>(
        &self,
        f: F,
    ) -> Result<(), SharedError>;

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

    ///  Rebuild output tree
    fn rebuild_tree(&self, r: &H256);
}

impl<'a, T: ChainStore> Iterator for ChainStoreHeaderIterator<'a, T> {
    type Item = Header;

    fn next(&mut self) -> Option<Self::Item> {
        let current_header = self.head.take();
        self.head = match current_header {
            Some(ref h) => {
                if h.number() > 0 {
                    self.store.get_header(&h.parent_hash())
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
            Some(ref h) => (1, Some(h.number() as usize + 1)),
            None => (0, Some(0)),
        }
    }
}

impl<T: 'static + KeyValueDB> ChainStore for ChainKVStore<T> {
    // TODO error log
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.get_header(h).map(|header| {
            let commit_transactions = self
                .get_block_body(h)
                .expect("block transactions must be stored");
            let uncles = self
                .get_block_uncles(h)
                .expect("block uncles must be stored");
            let proposal_transactions = self
                .get_block_proposal_txs_ids(h)
                .expect("block proposal_ids must be stored");
            BlockBuilder::default()
                .header(header)
                .uncles(uncles)
                .commit_transactions(commit_transactions)
                .proposal_transactions(proposal_transactions)
                .build()
        })
    }

    fn get_header(&self, h: &H256) -> Option<Header> {
        self.get(COLUMN_BLOCK_HEADER, h.as_bytes())
            .map(|ref raw| HeaderBuilder::new(raw).with_hash(h.clone()))
    }

    fn get_block_uncles(&self, h: &H256) -> Option<Vec<UncleBlock>> {
        // TODO Q use builder
        self.get(COLUMN_BLOCK_UNCLE, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>> {
        self.get(COLUMN_BLOCK_PROPOSAL_IDS, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.get(COLUMN_BLOCK_TRANSACTION_ADDRESSES, h.as_bytes())
            .and_then(|serialized_addresses| {
                let addresses: Vec<Address> = deserialize(&serialized_addresses).unwrap();
                self.get(COLUMN_BLOCK_BODY, h.as_bytes())
                    .and_then(|serialized_body| {
                        let txs: Vec<TransactionBuilder> = addresses
                            .iter()
                            .filter_map(|address| {
                                serialized_body
                                    .get(address.offset..(address.offset + address.length))
                                    .map(TransactionBuilder::new)
                            })
                            .collect();

                        self.get(COLUMN_BLOCK_TRANSACTION_IDS, h.as_bytes())
                            .map(|serialized_ids| (txs, serialized_ids))
                    })
            })
            .map(|(txs, serialized_ids)| {
                let txs_ids: Vec<H256> = deserialize(&serialized_ids[..]).unwrap();
                txs.into_iter()
                    .zip(txs_ids.into_iter())
                    .map(|(tx, id)| tx.with_hash(id))
                    .collect()
            })
    }

    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.get(COLUMN_EXT, block_hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_transaction_meta(&self, root: &H256, key: &H256) -> Option<TransactionMeta> {
        {
            let mut tree = self.tree.write();
            if tree.root_hash().as_ref() == Some(root) {
                return tree.get(key).unwrap_or(None);
            }
        }
        search(&*self.db, COLUMN_TRANSACTION_META, root.clone(), &key)
            .expect("tree operation error")
    }

    fn get_output_root(&self, block_hash: &H256) -> Option<H256> {
        self.get(COLUMN_OUTPUT_ROOT, block_hash.as_bytes())
            .map(|raw| H256::from_slice(&raw[..]).expect("db operation should be ok"))
    }

    fn update_transaction_meta(
        &self,
        batch: &mut Batch,
        root: H256,
        cells: Vec<(Vec<OutPoint>, Vec<OutPoint>)>,
    ) -> Option<H256> {
        //is mut reference to self.tree will end?
        let mut tree = self.tree.write();
        let mut new = AvlTree::new(
            Arc::<T>::clone(&self.db),
            COLUMN_TRANSACTION_META,
            root.clone(),
        );
        let avl = {
            if tree.root_hash() == Some(root) {
                &mut *tree
            } else {
                drop(tree);
                &mut new
            }
        };

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
                let hash = outputs[0].hash.clone();
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

        Some(avl.commit(batch))
    }

    fn rebuild_tree(&self, r: &H256) {
        let mut tree = self.tree.write();
        tree.reconstruct(r);
    }

    fn save_with_batch<F: FnOnce(&mut Batch) -> Result<(), SharedError>>(
        &self,
        f: F,
    ) -> Result<(), SharedError> {
        let mut batch = Batch::new();
        f(&mut batch)?;
        self.db.write(batch)?;
        Ok(())
    }

    fn insert_block(&self, batch: &mut Batch, b: &Block) {
        let hash = b.header().hash().to_vec();
        let txs_ids = b
            .commit_transactions()
            .iter()
            .map(|tx| tx.hash())
            .cloned()
            .collect::<Vec<H256>>();
        batch.insert(
            COLUMN_BLOCK_HEADER,
            hash.clone(),
            serialize(b.header()).expect("serializing header should be ok"),
        );
        let (block_data, block_addresses) = flat_serialize(b.commit_transactions().iter()).unwrap();
        batch.insert(
            COLUMN_BLOCK_TRANSACTION_IDS,
            hash.clone(),
            serialize(&txs_ids).expect("serializing txs hash should be ok"),
        );
        batch.insert(
            COLUMN_BLOCK_UNCLE,
            hash.clone(),
            serialize(b.uncles()).expect("serializing uncles should be ok"),
        );
        batch.insert(COLUMN_BLOCK_BODY, hash.clone(), block_data);
        batch.insert(
            COLUMN_BLOCK_PROPOSAL_IDS,
            hash.clone(),
            serialize(b.proposal_transactions())
                .expect("serializing proposal_transactions should be ok"),
        );
        batch.insert(
            COLUMN_BLOCK_TRANSACTION_ADDRESSES,
            hash,
            serialize(&block_addresses).expect("serializing addresses should be ok"),
        );
    }

    fn insert_block_ext(&self, batch: &mut Batch, block_hash: &H256, ext: &BlockExt) {
        batch.insert(COLUMN_EXT, block_hash.to_vec(), serialize(&ext).unwrap());
    }

    fn insert_output_root(&self, batch: &mut Batch, block_hash: &H256, r: &H256) {
        batch.insert(COLUMN_OUTPUT_ROOT, block_hash.to_vec(), r.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_db::diskdb::RocksDB;
    use tempfile;

    #[test]
    fn save_and_get_output_root() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("save_and_get_output_root")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore::new(db);

        assert!(store
            .save_with_batch(|batch| {
                store.insert_output_root(
                    batch,
                    &H256::from_trimmed_hex_str("10").unwrap(),
                    &H256::from_trimmed_hex_str("20").unwrap(),
                );
                Ok(())
            })
            .is_ok());
        assert_eq!(
            H256::from_trimmed_hex_str("20").unwrap(),
            store
                .get_output_root(&H256::from_trimmed_hex_str("10").unwrap())
                .unwrap()
        );
    }

    #[test]
    fn save_and_get_block() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("save_and_get_block")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();

        let hash = block.header().hash();
        assert!(store
            .save_with_batch(|batch| {
                store.insert_block(batch, &block);
                Ok(())
            })
            .is_ok());
        assert_eq!(block, &store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_with_transactions() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("save_and_get_block_with_transaction")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore::new(db);
        let block = BlockBuilder::default()
            .commit_transaction(TransactionBuilder::default().build())
            .commit_transaction(TransactionBuilder::default().build())
            .commit_transaction(TransactionBuilder::default().build())
            .build();

        let hash = block.header().hash();
        assert!(store
            .save_with_batch(|batch| {
                store.insert_block(batch, &block);
                Ok(())
            })
            .is_ok());
        assert_eq!(block, store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_ext() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("save_and_get_block_ext")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();

        let ext = BlockExt {
            received_at: block.header().timestamp(),
            total_difficulty: block.header().difficulty().clone(),
            total_uncles_count: block.uncles().len() as u64,
        };

        let hash = block.header().hash();

        assert!(store
            .save_with_batch(|batch| {
                store.insert_block_ext(batch, &hash, &ext);
                Ok(())
            })
            .is_ok());
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }
}
