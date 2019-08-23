use crate::{
    BLOCK_PROPOSALS_CACHE, BLOCK_TX_HASHES_CACHE, BLOCK_UNCLES_CACHE, CELLBASE_CACHE,
    CELL_DATA_CACHE, HEADER_CACHE,
};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL_SET, COLUMN_EPOCH, COLUMN_INDEX,
    COLUMN_META, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES, META_CURRENT_EPOCH_KEY,
    META_TIP_HEADER_KEY,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_db::{iter::DBIteratorItem, Col, Direction};
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::CellMeta, BlockExt, BlockNumber, BlockView, EpochExt, EpochNumber, HeaderView,
        TransactionInfo, TransactionMeta, TransactionView, UncleBlockVecView,
    },
    packed,
    prelude::*,
};

pub trait ChainStore<'a>: Send + Sync {
    type Vector: AsRef<[u8]>;
    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector>;
    fn get_iter<'i>(
        &'i self,
        col: Col,
        from_key: &'i [u8],
        direction: Direction,
    ) -> Box<Iterator<Item = DBIteratorItem> + 'i>;

    /// Get block by block header hash
    fn get_block(&'a self, h: &packed::Byte32) -> Option<BlockView> {
        self.get_block_header(h).map(|header| {
            let body = self.get_block_body(h);
            let uncles = self
                .get_block_uncles(h)
                .expect("block uncles must be stored");
            let proposals = self
                .get_block_proposal_txs_ids(h)
                .expect("block proposal_ids must be stored");
            BlockView::new_unchecked(header, uncles, body, proposals)
        })
    }

    /// Get header by block header hash
    fn get_block_header(&'a self, hash: &packed::Byte32) -> Option<HeaderView> {
        let mut ret: Option<HeaderView> = Default::default();
        let mut hit = false;
        HEADER_CACHE.with(|cache| {
            if let Some(header) = cache.borrow_mut().get_refresh(hash) {
                ret = Some(header.clone());
                hit = true;
            };
        });

        if hit {
            return ret;
        }

        ret = self.get(COLUMN_BLOCK_HEADER, hash.as_slice()).map(|slice| {
            let reader = packed::HeaderViewReader::from_slice(&slice.as_ref()).should_be_ok();
            Unpack::<HeaderView>::unpack(&reader)
        });

        if let Some(header) = ret.clone() {
            HEADER_CACHE.with(|cache| {
                cache.borrow_mut().insert(hash.clone(), header);
            });
        }

        ret
    }

    /// Get block body by block header hash
    fn get_block_body(&'a self, hash: &packed::Byte32) -> Vec<TransactionView> {
        let prefix = hash.as_slice();
        self.get_iter(COLUMN_BLOCK_BODY, prefix, Direction::Forward)
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(_key, value)| {
                let reader =
                    packed::TransactionViewReader::from_slice(&value.as_ref()).should_be_ok();
                Unpack::<TransactionView>::unpack(&reader)
            })
            .collect()
    }

    /// Get all transaction-hashes in block body by block header hash
    fn get_block_txs_hashes(&'a self, hash: &packed::Byte32) -> Vec<packed::Byte32> {
        let mut ret: Vec<packed::Byte32> = Default::default();
        let mut hit = false;
        BLOCK_TX_HASHES_CACHE.with(|cache| {
            if let Some(hashes) = cache.borrow_mut().get_refresh(hash) {
                ret = hashes.clone();
                hit = true;
            }
        });

        if hit {
            return ret;
        }

        let prefix = hash.as_slice();
        let ret: Vec<_> = self
            .get_iter(COLUMN_BLOCK_BODY, prefix, Direction::Forward)
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(_key, value)| {
                let reader =
                    packed::TransactionViewReader::from_slice(&value.as_ref()).should_be_ok();
                reader.hash().to_entity()
            })
            .collect();

        BLOCK_TX_HASHES_CACHE.with(|cache| {
            cache.borrow_mut().insert(hash.clone(), ret.clone());
        });

        ret
    }

    /// Get proposal short id by block header hash
    fn get_block_proposal_txs_ids(
        &'a self,
        hash: &packed::Byte32,
    ) -> Option<packed::ProposalShortIdVec> {
        let mut ret: Option<packed::ProposalShortIdVec> = Default::default();
        let mut hit = false;
        BLOCK_PROPOSALS_CACHE.with(|cache| {
            if let Some(data) = cache.borrow_mut().get_refresh(hash) {
                ret = Some(data.clone());
                hit = true;
            }
        });

        if hit {
            return ret;
        }

        ret = self
            .get(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())
            .map(|slice| {
                packed::ProposalShortIdVecReader::from_slice(&slice.as_ref())
                    .should_be_ok()
                    .to_entity()
            });

        if let Some(data) = ret.clone() {
            BLOCK_PROPOSALS_CACHE.with(|cache| {
                cache.borrow_mut().insert(hash.clone(), data);
            });
        }

        ret
    }

    /// Get block uncles by block header hash
    fn get_block_uncles(&'a self, hash: &packed::Byte32) -> Option<UncleBlockVecView> {
        let mut ret: Option<UncleBlockVecView> = Default::default();
        let mut hit = false;

        BLOCK_UNCLES_CACHE.with(|cache| {
            if let Some(data) = cache.borrow_mut().get_refresh(&hash) {
                ret = Some(data.clone());
                hit = true;
            }
        });

        if hit {
            return ret;
        }

        ret = self.get(COLUMN_BLOCK_UNCLE, hash.as_slice()).map(|slice| {
            let reader =
                packed::UncleBlockVecViewReader::from_slice(&slice.as_ref()).should_be_ok();
            Unpack::<UncleBlockVecView>::unpack(&reader)
        });

        if let Some(uncles) = ret.clone() {
            BLOCK_UNCLES_CACHE.with(|cache| {
                cache.borrow_mut().insert(hash.clone(), uncles);
            });
        }

        ret
    }

    /// Get block ext by block header hash
    fn get_block_ext(&'a self, block_hash: &packed::Byte32) -> Option<BlockExt> {
        self.get(COLUMN_BLOCK_EXT, block_hash.as_slice())
            .map(|slice| {
                packed::BlockExtReader::from_slice(&slice.as_ref()[..])
                    .should_be_ok()
                    .unpack()
            })
    }

    /// Get block header hash by block number
    fn get_block_hash(&'a self, number: BlockNumber) -> Option<packed::Byte32> {
        let block_number: packed::Uint64 = number.pack();
        self.get(COLUMN_INDEX, block_number.as_slice()).map(|raw| {
            packed::Byte32Reader::from_slice(&raw.as_ref()[..])
                .should_be_ok()
                .to_entity()
        })
    }

    /// Get block number by block header hash
    fn get_block_number(&'a self, hash: &packed::Byte32) -> Option<BlockNumber> {
        self.get(COLUMN_INDEX, hash.as_slice()).map(|raw| {
            packed::Uint64Reader::from_slice(&raw.as_ref()[..])
                .should_be_ok()
                .unpack()
        })
    }

    fn get_tip_header(&'a self) -> Option<HeaderView> {
        self.get(COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|raw| {
                self.get_block_header(
                    &packed::Byte32Reader::from_slice(&raw.as_ref()[..])
                        .should_be_ok()
                        .to_entity(),
                )
            })
            .map(Into::into)
    }

    /// Get commit transaction and block hash by it's hash
    fn get_transaction(
        &'a self,
        hash: &packed::Byte32,
    ) -> Option<(TransactionView, packed::Byte32)> {
        self.get_transaction_info_packed(hash).map(|info| {
            self.get(COLUMN_BLOCK_BODY, info.key().as_slice())
                .map(|slice| {
                    let reader =
                        packed::TransactionViewReader::from_slice(&slice.as_ref()).should_be_ok();
                    let hash = info.as_reader().key().block_hash().to_entity();
                    (reader.unpack(), hash)
                })
                .expect("since tx info is existed, so tx data should be existed")
        })
    }

    fn get_transaction_info_packed(
        &'a self,
        hash: &packed::Byte32,
    ) -> Option<packed::TransactionInfo> {
        self.get(COLUMN_TRANSACTION_INFO, hash.as_slice())
            .map(|slice| {
                let reader =
                    packed::TransactionInfoReader::from_slice(&slice.as_ref()).should_be_ok();
                reader.to_entity()
            })
    }

    fn get_transaction_info(&'a self, hash: &packed::Byte32) -> Option<TransactionInfo> {
        self.get(COLUMN_TRANSACTION_INFO, hash.as_slice())
            .map(|slice| {
                let reader =
                    packed::TransactionInfoReader::from_slice(&slice.as_ref()).should_be_ok();
                Unpack::<TransactionInfo>::unpack(&reader)
            })
    }

    fn get_tx_meta(&'a self, tx_hash: &packed::Byte32) -> Option<TransactionMeta> {
        self.get(COLUMN_CELL_SET, tx_hash.as_slice()).map(|slice| {
            packed::TransactionMetaReader::from_slice(&slice.as_ref())
                .should_be_ok()
                .unpack()
        })
    }

    fn get_cell_meta(&'a self, tx_hash: &packed::Byte32, index: u32) -> Option<CellMeta> {
        self.get_transaction_info_packed(&tx_hash)
            .and_then(|tx_info| {
                self.get(COLUMN_BLOCK_BODY, tx_info.key().as_slice())
                    .map(|slice| {
                        let reader = packed::TransactionViewReader::from_slice(&slice.as_ref())
                            .should_be_ok();
                        let cell_output = reader
                            .data()
                            .slim()
                            .raw()
                            .outputs()
                            .get(index as usize)
                            .expect("inconsistent index")
                            .to_entity();
                        let data_bytes = reader
                            .data()
                            .outputs_data()
                            .get(index as usize)
                            .expect("inconsistent index")
                            .raw_data()
                            .len() as u64;
                        let out_point = packed::OutPoint::new_builder()
                            .tx_hash(tx_hash.to_owned())
                            .index(index.pack())
                            .build();
                        // notice mem_cell_data is set to None, the cell data should be load in need
                        CellMeta {
                            cell_output,
                            out_point,
                            transaction_info: Some(tx_info.unpack()),
                            data_bytes,
                            mem_cell_data: None,
                        }
                    })
            })
    }

    fn get_cell_data(&'a self, tx_hash: &packed::Byte32, index: u32) -> Option<Bytes> {
        let mut ret: Option<Bytes> = Default::default();
        let mut hit = false;
        CELL_DATA_CACHE.with(|cache| {
            if let Some(data) = cache.borrow_mut().get_refresh(&(tx_hash.clone(), index)) {
                ret = Some(data.clone());
                hit = true;
            };
        });

        if hit {
            return ret;
        }

        ret = self.get_transaction_info_packed(tx_hash).and_then(|info| {
            self.get(COLUMN_BLOCK_BODY, info.key().as_slice())
                .and_then(|slice| {
                    let reader =
                        packed::TransactionViewReader::from_slice(&slice.as_ref()).should_be_ok();
                    reader
                        .data()
                        .outputs_data()
                        .get(index as usize)
                        .map(|data| Unpack::<Bytes>::unpack(&data))
                })
        });

        if let Some(data) = ret.clone() {
            CELL_DATA_CACHE.with(|cache| {
                cache.borrow_mut().insert((tx_hash.clone(), index), data);
            });
        }

        ret
    }

    // Get current epoch ext
    fn get_current_epoch_ext(&'a self) -> Option<EpochExt> {
        self.get(COLUMN_META, META_CURRENT_EPOCH_KEY).map(|slice| {
            packed::EpochExtReader::from_slice(&slice.as_ref())
                .should_be_ok()
                .unpack()
        })
    }

    // Get epoch ext by epoch index
    fn get_epoch_ext(&'a self, hash: &packed::Byte32) -> Option<EpochExt> {
        self.get(COLUMN_EPOCH, hash.as_slice()).map(|slice| {
            packed::EpochExtReader::from_slice(&slice.as_ref())
                .should_be_ok()
                .unpack()
        })
    }

    // Get epoch index by epoch number
    fn get_epoch_index(&'a self, number: EpochNumber) -> Option<packed::Byte32> {
        let epoch_number: packed::Uint64 = number.pack();
        self.get(COLUMN_EPOCH, epoch_number.as_slice()).map(|raw| {
            packed::Byte32Reader::from_slice(&raw.as_ref())
                .should_be_ok()
                .to_entity()
        })
    }

    // Get epoch index by block hash
    fn get_block_epoch_index(&'a self, block_hash: &packed::Byte32) -> Option<packed::Byte32> {
        self.get(COLUMN_BLOCK_EPOCH, block_hash.as_slice())
            .map(|raw| {
                packed::Byte32Reader::from_slice(&raw.as_ref())
                    .should_be_ok()
                    .to_entity()
            })
    }

    fn get_block_epoch(&'a self, hash: &packed::Byte32) -> Option<EpochExt> {
        self.get_block_epoch_index(hash)
            .and_then(|index| self.get_epoch_ext(&index))
    }

    fn is_uncle(&'a self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_UNCLES, hash.as_slice()).is_some()
    }

    fn block_exists(&'a self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_BLOCK_HEADER, hash.as_slice()).is_some()
    }

    // Get cellbase by block hash
    fn get_cellbase(&'a self, hash: &packed::Byte32) -> Option<TransactionView> {
        let mut ret: Option<TransactionView> = Default::default();
        let mut hit = false;

        CELLBASE_CACHE.with(|cache| {
            if let Some(data) = cache.borrow_mut().get_refresh(&hash) {
                ret = Some(data.clone());
                hit = true;
            }
        });

        if hit {
            return ret;
        }

        let key = packed::TransactionKey::new_builder()
            .block_hash(hash.to_owned())
            .build();
        let ret = self.get(COLUMN_BLOCK_BODY, key.as_slice()).map(|slice| {
            let reader = packed::TransactionViewReader::from_slice(&slice.as_ref()).should_be_ok();
            Unpack::<TransactionView>::unpack(&reader)
        });

        if let Some(data) = ret.clone() {
            CELLBASE_CACHE.with(|cache| {
                cache.borrow_mut().insert(hash.clone(), data);
            })
        }

        ret
    }

    fn next_epoch_ext(
        &'a self,
        consensus: &Consensus,
        last_epoch: &EpochExt,
        header: &HeaderView,
    ) -> Option<EpochExt> {
        consensus.next_epoch_ext(
            last_epoch,
            header,
            |hash| self.get_block_header(&hash),
            |hash| self.get_block_ext(&hash).map(|ext| ext.total_uncles_count),
        )
    }

    fn get_ancestor(&'a self, base: &packed::Byte32, number: BlockNumber) -> Option<HeaderView> {
        if let Some(header) = self.get_block_header(base) {
            let mut n_number: BlockNumber = header.data().raw().number().unpack();
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.get_block_header(&index_walk.data().raw().parent_hash())
                {
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
}
