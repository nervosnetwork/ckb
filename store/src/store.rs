use crate::{CELL_DATA_CACHE, HEADER_CACHE};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL_SET, COLUMN_EPOCH, COLUMN_INDEX,
    COLUMN_META, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES, META_CURRENT_EPOCH_KEY,
    META_TIP_HEADER_KEY,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{BlockInfo, CellMeta};
use ckb_core::extras::{BlockExt, EpochExt, TransactionInfo};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{CellOutPoint, ProposalShortId, Transaction};
use ckb_core::transaction_meta::TransactionMeta;
use ckb_core::uncle::UncleBlock;
use ckb_core::{Bytes, EpochNumber};
use ckb_db::Col;
use ckb_protos as protos;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub trait ChainStore<'a>: Send + Sync {
    type Vector: AsRef<[u8]>;
    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector>;

    /// Get block by block header hash
    fn get_block(&'a self, h: &H256) -> Option<Block> {
        self.get_block_header(h).map(|header| {
            let transactions = self
                .get_block_body(h)
                .expect("block transactions must be stored");
            let uncles = self
                .get_block_uncles(h)
                .expect("block uncles must be stored");
            let proposals = self
                .get_block_proposal_txs_ids(h)
                .expect("block proposal_ids must be stored");
            BlockBuilder::default()
                .header(header)
                .uncles(uncles)
                .transactions(transactions)
                .proposals(proposals)
                .build()
        })
    }

    /// Get header by block header hash
    fn get_block_header(&'a self, hash: &H256) -> Option<Header> {
        {
            if let Some(header) = HEADER_CACHE.lock().get_refresh(hash) {
                return Some(header.clone());
            }
        }
        self.get(COLUMN_BLOCK_HEADER, hash.as_bytes())
            .map(|slice| {
                protos::StoredHeader::from_slice(&slice.as_ref())
                    .try_into()
                    .expect("deserialize")
            })
            .map(|header: Header| {
                HEADER_CACHE.lock().insert(hash.clone(), header.clone());
                header
            })
    }

    /// Get block body by block header hash
    fn get_block_body(&'a self, hash: &H256) -> Option<Vec<Transaction>> {
        self.get(COLUMN_BLOCK_BODY, hash.as_bytes()).map(|slice| {
            protos::StoredBlockBody::from_slice(&slice.as_ref())
                .try_into()
                .expect("deserialize")
        })
    }

    /// Get all transaction-hashes in block body by block header hash
    fn get_block_txs_hashes(&'a self, hash: &H256) -> Option<Vec<H256>> {
        self.get(COLUMN_BLOCK_BODY, hash.as_bytes()).map(|slice| {
            protos::StoredBlockBody::from_slice(&slice.as_ref())
                .tx_hashes()
                .expect("deserialize")
        })
    }

    /// Get proposal short id by block header hash
    fn get_block_proposal_txs_ids(&'a self, hash: &H256) -> Option<Vec<ProposalShortId>> {
        self.get(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_bytes())
            .map(|slice| {
                protos::StoredProposalShortIds::from_slice(&slice.as_ref())
                    .try_into()
                    .expect("deserialize")
            })
    }

    /// Get block uncles by block header hash
    fn get_block_uncles(&'a self, hash: &H256) -> Option<Vec<UncleBlock>> {
        self.get(COLUMN_BLOCK_UNCLE, hash.as_bytes()).map(|slice| {
            protos::StoredUncleBlocks::from_slice(&slice.as_ref())
                .try_into()
                .expect("deserialize")
        })
    }

    /// Get block ext by block header hash
    fn get_block_ext(&'a self, block_hash: &H256) -> Option<BlockExt> {
        self.get(COLUMN_BLOCK_EXT, block_hash.as_bytes())
            .map(|slice| {
                protos::BlockExt::from_slice(&slice.as_ref()[..])
                    .try_into()
                    .expect("deserialize")
            })
    }

    /// Get block header hash by block number
    fn get_block_hash(&'a self, number: BlockNumber) -> Option<H256> {
        self.get(COLUMN_INDEX, &number.to_le_bytes())
            .map(|raw| H256::from_slice(&raw.as_ref()[..]).expect("db safe access"))
    }

    /// Get block number by block header hash
    fn get_block_number(&'a self, hash: &H256) -> Option<BlockNumber> {
        self.get(COLUMN_INDEX, hash.as_bytes()).map(|raw| {
            let le_bytes: [u8; 8] = raw.as_ref()[..].try_into().expect("should not be failed");
            u64::from_le_bytes(le_bytes)
        })
    }

    fn get_tip_header(&'a self) -> Option<Header> {
        self.get(COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|raw| {
                self.get_block_header(&H256::from_slice(&raw.as_ref()[..]).expect("db safe access"))
            })
            .map(Into::into)
    }

    /// Get commit transaction and block hash by it's hash
    fn get_transaction(&'a self, hash: &H256) -> Option<(Transaction, H256)> {
        self.get_transaction_info(&hash).and_then(|info| {
            self.get(COLUMN_BLOCK_BODY, info.block_hash.as_bytes())
                .and_then(|slice| {
                    protos::StoredBlockBody::from_slice(&slice.as_ref())
                        .transaction(info.index)
                        .expect("deserialize")
                        .map(|tx| (tx, info.block_hash))
                })
        })
    }

    fn get_transaction_info(&'a self, hash: &H256) -> Option<TransactionInfo> {
        self.get(COLUMN_TRANSACTION_INFO, hash.as_bytes())
            .map(|slice| {
                protos::StoredTransactionInfo::from_slice(&slice.as_ref())
                    .try_into()
                    .expect("deserialize")
            })
    }

    fn get_tx_meta(&'a self, tx_hash: &H256) -> Option<TransactionMeta> {
        self.get(COLUMN_CELL_SET, tx_hash.as_bytes()).map(|slice| {
            protos::TransactionMeta::from_slice(&slice.as_ref())
                .try_into()
                .expect("deserialize")
        })
    }

    fn get_cell_meta(&'a self, tx_hash: &H256, index: u32) -> Option<CellMeta> {
        self.get_transaction_info(&tx_hash)
            .and_then(|tx_info| {
                let tx_info_index = tx_info.index;
                self.get(COLUMN_BLOCK_BODY, tx_info.block_hash.as_bytes())
                    .and_then(|slice| {
                        let deserialized = protos::StoredBlockBody::from_slice(&slice.as_ref());
                        let cell_output = deserialized
                            .output(tx_info_index, index as usize)
                            .expect("db operation should be ok");
                        let data = deserialized
                            .output_data(tx_info_index, index as usize)
                            .expect("db operation should be ok");
                        cell_output
                            .and_then(|cell_output| data.map(|data| (tx_info, cell_output, data)))
                    })
            })
            .map(|(tx_info, cell_output, data)| {
                let out_point = CellOutPoint {
                    tx_hash: tx_hash.to_owned(),
                    index: index as u32,
                };
                let cellbase = tx_info.index == 0;
                let block_info = BlockInfo {
                    number: tx_info.block_number,
                    epoch: tx_info.block_epoch,
                    hash: tx_info.block_hash,
                };
                // notice mem_cell_data is set to None, the cell data should be load in need
                CellMeta {
                    cell_output,
                    out_point,
                    block_info: Some(block_info),
                    cellbase,
                    data_bytes: data.len() as u64,
                    mem_cell_data: None,
                }
            })
    }

    fn get_cell_data(&'a self, tx_hash: &H256, index: u32) -> Option<Bytes> {
        {
            if let Some(data) = CELL_DATA_CACHE
                .lock()
                .get_refresh(&(tx_hash.clone(), index))
            {
                return Some(data.clone());
            }
        }

        self.get_transaction_info(&tx_hash)
            .and_then(|info| {
                self.get(COLUMN_BLOCK_BODY, info.block_hash.as_bytes())
                    .and_then(|slice| {
                        protos::StoredBlockBody::from_slice(&slice.as_ref())
                            .output_data(info.index, index as usize)
                            .expect("db operation should be ok")
                    })
            })
            .map(|data: Bytes| {
                CELL_DATA_CACHE
                    .lock()
                    .insert((tx_hash.clone(), index), data.clone());
                data
            })
    }

    // Get current epoch ext
    fn get_current_epoch_ext(&'a self) -> Option<EpochExt> {
        self.get(COLUMN_META, META_CURRENT_EPOCH_KEY).map(|slice| {
            protos::StoredEpochExt::from_slice(&slice.as_ref())
                .try_into()
                .expect("deserialize")
        })
    }

    // Get epoch ext by epoch index
    fn get_epoch_ext(&'a self, hash: &H256) -> Option<EpochExt> {
        self.get(COLUMN_EPOCH, hash.as_bytes()).map(|slice| {
            protos::StoredEpochExt::from_slice(&slice.as_ref())
                .try_into()
                .expect("deserialize")
        })
    }

    // Get epoch index by epoch number
    fn get_epoch_index(&'a self, number: EpochNumber) -> Option<H256> {
        self.get(COLUMN_EPOCH, &number.to_le_bytes())
            .map(|raw| H256::from_slice(&raw.as_ref()).expect("db safe access"))
    }

    // Get epoch index by block hash
    fn get_block_epoch_index(&'a self, block_hash: &H256) -> Option<H256> {
        self.get(COLUMN_BLOCK_EPOCH, block_hash.as_bytes())
            .map(|raw| H256::from_slice(&raw.as_ref()).expect("db safe access"))
    }

    fn get_block_epoch(&'a self, hash: &H256) -> Option<EpochExt> {
        self.get_block_epoch_index(hash)
            .and_then(|index| self.get_epoch_ext(&index))
    }

    fn is_uncle(&'a self, hash: &H256) -> bool {
        self.get(COLUMN_UNCLES, hash.as_bytes()).is_some()
    }

    fn block_exists(&'a self, hash: &H256) -> bool {
        self.get(COLUMN_BLOCK_HEADER, hash.as_bytes()).is_some()
    }

    // Get cellbase by block hash
    fn get_cellbase(&'a self, hash: &H256) -> Option<Transaction> {
        self.get(COLUMN_BLOCK_BODY, hash.as_bytes())
            .and_then(|slice| {
                protos::StoredBlockBody::from_slice(&slice.as_ref())
                    .transaction(0)
                    .expect("cellbase address should exist")
            })
    }

    fn next_epoch_ext(
        &'a self,
        consensus: &Consensus,
        last_epoch: &EpochExt,
        header: &Header,
    ) -> Option<EpochExt> {
        consensus.next_epoch_ext(
            last_epoch,
            header,
            |hash| self.get_block_header(hash),
            |hash| self.get_block_ext(hash).map(|ext| ext.total_uncles_count),
        )
    }

    fn get_ancestor(&'a self, base: &H256, number: BlockNumber) -> Option<Header> {
        if let Some(header) = self.get_block_header(base) {
            let mut n_number = header.number();
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.get_block_header(&index_walk.parent_hash()) {
                    index_walk = header;
                    n_number -= 1;
                } else {
                    return None;
                }
            }
            return Some(index_walk);
        }
        None
    }

    // Only for test
    fn clear_header_cache(&'a self) {
        HEADER_CACHE.lock().clear();
    }

    // Only for test
    fn clear_cell_data_cache(&'a self) {
        CELL_DATA_CACHE.lock().clear();
    }
}
