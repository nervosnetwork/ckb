use crate::error::SharedError;
use crate::flat_serializer::{serialize as flat_serialize, Address};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_TRANSACTION_ADDRESSES, COLUMN_BLOCK_TRANSACTION_IDS, COLUMN_BLOCK_UNCLE,
    COLUMN_EXT, COLUMN_META,
};
use bincode::{deserialize, serialize};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::extras::BlockExt;
use ckb_core::header::{BlockNumber, Header, HeaderBuilder};
use ckb_core::transaction::{ProposalShortId, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_db::batch::{Batch, Col};
use ckb_db::kvdb::KeyValueDB;
use ckb_util::RwLock;
use numext_fixed_hash::H256;
use std::ops::Range;
use std::sync::Arc;

const META_TIP_HASH_KEY: &[u8] = b"TIP_HASH";
const META_TIP_NUMBER_KEY: &[u8] = b"TIP_NUMBER";

#[derive(Default)]
pub struct ChainTip {
    pub(crate) number: BlockNumber,
    pub(crate) hash: H256,
}

pub struct ChainKVStore<T: KeyValueDB> {
    pub(crate) tip: RwLock<ChainTip>,
    db: Arc<T>,
}

impl<T: 'static + KeyValueDB> ChainKVStore<T> {
    pub fn new(db: T) -> Self {
        let tip_number = db
            .read(COLUMN_META, META_TIP_NUMBER_KEY)
            .expect("new db")
            .map(|raw| deserialize(&raw[..]).expect("invalid tip number"));
        let tip_hash = db
            .read(COLUMN_META, META_TIP_HASH_KEY)
            .expect("new db")
            .map(|raw| H256::from_slice(&raw[..]).expect("invalid tip hash"));

        let tip = if let (Some(number), Some(hash)) = (tip_number, tip_hash) {
            ChainTip { number, hash }
        } else {
            Default::default()
        };

        ChainKVStore {
            tip: RwLock::new(tip),
            db: Arc::new(db),
        }
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
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>>;
    fn get_block_uncles(&self, block_hash: &H256) -> Option<Vec<UncleBlock>>;
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;
    fn insert_block(&self, batch: &mut Batch, b: &Block);
    fn insert_block_ext(&self, batch: &mut Batch, block_hash: &H256, ext: &BlockExt);
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
            .map(|ref raw| HeaderBuilder::new(raw).build())
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
                    .map(|serialized_body| {
                        let txs: Vec<TransactionBuilder> = addresses
                            .iter()
                            .filter_map(|address| {
                                serialized_body
                                    .get(address.offset..(address.offset + address.length))
                                    .map(TransactionBuilder::new)
                            })
                            .collect();

                        txs
                    })
            })
            .map(|txs| txs.into_iter().map(|tx| tx.build()).collect())
    }

    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.get(COLUMN_EXT, block_hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
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
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_db::diskdb::RocksDB;
    use tempfile;

    #[test]
<<<<<<< HEAD
    fn save_and_get_output_root() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("save_and_get_output_root")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore::new(db);

        let ret = store.save_with_batch(|batch| {
            store.insert_output_root(
                batch,
                &H256::from_trimmed_hex_str("10").unwrap(),
                &H256::from_trimmed_hex_str("20").unwrap(),
            );
            Ok(())
        });
        assert!(ret.is_ok());
        assert_eq!(
            H256::from_trimmed_hex_str("20").unwrap(),
            store
                .get_output_root(&H256::from_trimmed_hex_str("10").unwrap())
                .unwrap()
        );
    }

    #[test]
=======
>>>>>>> remove avl
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
        let ret = store.save_with_batch(|batch| {
            store.insert_block(batch, &block);
            Ok(())
        });
        assert!(ret.is_ok());
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
        let ret = store.save_with_batch(|batch| {
            store.insert_block(batch, &block);
            Ok(())
        });
        assert!(ret.is_ok());
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
            valid: Some(true),
        };

        let hash = block.header().hash();
        let ret = store.save_with_batch(|batch| {
            store.insert_block_ext(batch, &hash, &ext);
            Ok(())
        });

        assert!(ret.is_ok());
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }
}
