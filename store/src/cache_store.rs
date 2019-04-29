use crate::ChainStore;
use ckb_core::block::Block;
use ckb_core::cell::CellMeta;
use ckb_core::extras::{BlockExt, TransactionAddress};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{CellOutput, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_db::Error;
use ckb_util::Mutex;
use lru_cache::LruCache;
use numext_fixed_hash::H256;

pub struct CacheStore<T> {
    db: T,
    cell_output_cache: Mutex<LruCache<(H256, u32), CellOutput>>,
}

impl<T: ChainStore> CacheStore<T> {
    pub fn new(db: T, cell_output_cache: Mutex<LruCache<(H256, u32), CellOutput>>) -> Self {
        CacheStore {
            db,
            cell_output_cache,
        }
    }
}

impl<T: ChainStore> ChainStore for CacheStore<T> {
    type Batch = T::Batch;

    #[inline]
    fn new_batch(&self) -> Result<Self::Batch, Error> {
        self.db.new_batch()
    }

    #[inline]
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.db.get_block(h)
    }

    #[inline]
    fn get_header(&self, h: &H256) -> Option<Header> {
        self.db.get_header(h)
    }

    #[inline]
    fn get_block_uncles(&self, h: &H256) -> Option<Vec<UncleBlock>> {
        self.db.get_block_uncles(h)
    }

    #[inline]
    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>> {
        self.db.get_block_proposal_txs_ids(h)
    }

    #[inline]
    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.db.get_block_body(h)
    }

    #[inline]
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.db.get_block_ext(block_hash)
    }

    #[inline]
    fn init(&self, genesis: &Block) -> Result<(), Error> {
        self.db.init(genesis)
    }

    #[inline]
    fn get_block_hash(&self, number: BlockNumber) -> Option<H256> {
        self.db.get_block_hash(number)
    }

    #[inline]
    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.db.get_block_number(hash)
    }

    #[inline]
    fn get_tip_header(&self) -> Option<Header> {
        self.db.get_tip_header()
    }

    #[inline]
    fn get_transaction(&self, h: &H256) -> Option<(Transaction, H256)> {
        self.db.get_transaction(h)
    }

    #[inline]
    fn get_transaction_address(&self, h: &H256) -> Option<TransactionAddress> {
        self.db.get_transaction_address(h)
    }

    #[inline]
    fn get_cell_meta(&self, tx_hash: &H256, index: u32) -> Option<CellMeta> {
        self.db.get_cell_meta(tx_hash, index)
    }

    #[inline]
    fn get_cell_output(&self, tx_hash: &H256, index: u32) -> Option<CellOutput> {
        let key = (tx_hash.to_owned(), index);
        let mut cache = self.cell_output_cache.lock();
        match cache.get_refresh(&key) {
            Some(cell_output) => Some(cell_output.to_owned()),
            None => {
                let cell_output = self.db.get_cell_output(tx_hash, index);
                if let Some(cell_output) = cell_output.as_ref() {
                    cache.insert(key, cell_output.to_owned());
                }
                cell_output
            }
        }
    }
}
