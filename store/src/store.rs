use crate::cache::StoreCache;
use crate::data_loader_wrapper::BorrowedDataLoaderWrapper;
use ckb_db::{
    iter::{DBIter, Direction, IteratorMode},
    DBPinnableSlice,
};
use ckb_db_schema::{
    Col, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_EXTENSION,
    COLUMN_BLOCK_FILTER, COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_HEADER_NUM,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA,
    COLUMN_CELL_DATA_HASH, COLUMN_CHAIN_ROOT_MMR, COLUMN_EPOCH, COLUMN_INDEX, COLUMN_META,
    COLUMN_TRANSACTION_INFO, COLUMN_UNCLES,
};
use ckb_freezer::Freezer;
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::CellMeta, BlockExt, BlockNumber, BlockView, EpochExt, EpochNumber, HeaderView,
        TransactionInfo, TransactionView, UncleBlockVecView,
    },
    packed::{self, OutPoint},
    prelude::*,
    BlockNumberAndHash,
};

/// The `ChainStore` trait provides chain data store interface
pub trait ChainStore: Send + Sync + Sized {
    /// Return cache reference
    fn cache(&self) -> Option<&StoreCache>;
    /// Return freezer reference
    fn freezer(&self) -> Option<&Freezer>;
    /// Return the bytes associated with a key value and the given column family.
    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice>;
    /// Return an iterator over the database key-value pairs in the given column family.
    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter;
    /// Return the borrowed data loader wrapper
    fn borrow_as_data_loader(&self) -> BorrowedDataLoaderWrapper<Self> {
        BorrowedDataLoaderWrapper::new(self)
    }

    /// Get block by block header hash
    fn get_block(&self, h: &packed::Byte32) -> Option<BlockView> {
        let header = self.get_block_header(h)?;
        let num_hash = header.num_hash();
        if let Some(freezer) = self.freezer() {
            if header.number() > 0 && header.number() < freezer.number() {
                let raw_block = freezer.retrieve(header.number()).expect("block frozen")?;
                let raw_block = packed::BlockReader::from_compatible_slice(&raw_block)
                    .expect("checked data")
                    .to_entity();
                return Some(raw_block.into_view());
            }
        }
        let body = self.get_block_body_by_num_hash(num_hash.clone());
        let uncles = self
            .get_block_uncles(num_hash.clone())
            .expect("block uncles must be stored");
        let proposals = self
            .get_block_proposal_txs_ids(num_hash)
            .expect("block proposal_ids must be stored");
        let extension_opt = self.get_block_extension(h);

        let block = if let Some(extension) = extension_opt {
            BlockView::new_unchecked_with_extension(header, uncles, body, proposals, extension)
        } else {
            BlockView::new_unchecked(header, uncles, body, proposals)
        };
        Some(block)
    }

    /// Get header by block header hash
    fn get_block_header(&self, hash: &packed::Byte32) -> Option<HeaderView> {
        if let Some(cache) = self.cache() {
            if let Some(header) = cache.headers.lock().get(hash) {
                return Some(header.clone());
            }
        };
        let ret = self
            .get_packed_block_header(hash)
            .map(|header| header.into_view());

        if let Some(cache) = self.cache() {
            ret.map(|header| {
                cache.headers.lock().put(hash.clone(), header.clone());
                header
            })
        } else {
            ret
        }
    }

    /// Get block body by block header hash
    fn get_block_body(&self, num_hash: BlockNumberAndHash) -> Vec<TransactionView> {
        // let num_hash = BlockNumberAndHash::new(number, hash.clone());
        // let num_hash = num_hash.to_db_key();
        // let prefix: &[u8] = num_hash.as_slice();
        // let prefix = column_block_body_prefix_key(number, hash).as_ref();
        let prefix = COLUMN_BLOCK_BODY::prefix_key(num_hash);

        self.get_iter(
            COLUMN_BLOCK_BODY::NAME,
            IteratorMode::From(prefix.as_ref(), Direction::Forward),
        )
        .take_while(|(key, _)| key.starts_with(prefix.as_ref()))
        .map(|(_key, value)| {
            let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
            Unpack::<TransactionView>::unpack(&reader)
        })
        .collect()
    }

    /// Get block body by number and hash
    fn get_block_body_by_num_hash(&self, num_hash: BlockNumberAndHash) -> Vec<TransactionView> {
        let prefix = COLUMN_BLOCK_BODY::prefix_key(num_hash);
        self.get_iter(
            COLUMN_BLOCK_BODY::NAME,
            IteratorMode::From(prefix.as_ref(), Direction::Forward),
        )
        .take_while(|(key, _)| key.starts_with(prefix.as_ref()))
        .map(|(_key, value)| {
            let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
            Unpack::<TransactionView>::unpack(&reader)
        })
        .collect()
    }

    /// Get unfrozen block from ky-store with given hash
    fn get_unfrozen_block(&self, hash: &packed::Byte32) -> Option<BlockView> {
        let header = self
            .get(COLUMN_BLOCK_HEADER::NAME, hash.as_slice())
            .map(|slice| {
                let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
                Unpack::<HeaderView>::unpack(&reader)
            })?;
        let num_hash = header.num_hash();

        let body = self.get_block_body(num_hash.clone());

        let uncles = self
            .get(
                COLUMN_BLOCK_UNCLE::NAME,
                COLUMN_BLOCK_UNCLE::key(num_hash.clone()).as_ref(),
            )
            .map(|slice| {
                let reader =
                    packed::UncleBlockVecViewReader::from_slice_should_be_ok(slice.as_ref());
                Unpack::<UncleBlockVecView>::unpack(&reader)
            })
            .expect("block uncles must be stored");

        let proposals = self
            .get(
                COLUMN_BLOCK_PROPOSAL_IDS::NAME,
                COLUMN_BLOCK_PROPOSAL_IDS::key(num_hash.clone()).as_ref(),
            )
            .map(|slice| {
                packed::ProposalShortIdVecReader::from_slice_should_be_ok(slice.as_ref())
                    .to_entity()
            })
            .expect("block proposal_ids must be stored");

        let extension_opt = self
            .get(COLUMN_BLOCK_EXTENSION::NAME, hash.as_slice())
            .map(|slice| packed::BytesReader::from_slice_should_be_ok(slice.as_ref()).to_entity());

        let block = if let Some(extension) = extension_opt {
            BlockView::new_unchecked_with_extension(header, uncles, body, proposals, extension)
        } else {
            BlockView::new_unchecked(header, uncles, body, proposals)
        };

        Some(block)
    }

    /// Get all transaction-hashes in block body by block header hash
    fn get_block_txs_hashes(&self, hash: &packed::Byte32) -> Vec<packed::Byte32> {
        if let Some(cache) = self.cache() {
            if let Some(hashes) = cache.block_tx_hashes.lock().get(hash) {
                return hashes.clone();
            }
        };
        let block_number = self.get_block_number(hash).expect("block number");
        let num_hash = BlockNumberAndHash::new(block_number, hash.to_owned());

        let prefix = COLUMN_BLOCK_BODY::prefix_key(num_hash);

        let ret: Vec<_> = self
            .get_iter(
                COLUMN_BLOCK_BODY::NAME,
                IteratorMode::From(prefix.as_ref(), Direction::Forward),
            )
            .take_while(|(key, _)| key.starts_with(prefix.as_ref()))
            .map(|(_key, value)| {
                let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
                reader.hash().to_entity()
            })
            .collect();

        if let Some(cache) = self.cache() {
            cache.block_tx_hashes.lock().put(hash.clone(), ret.clone());
        }

        ret
    }

    /// Get proposal short id by block header hash
    fn get_block_proposal_txs_ids(
        &self,
        num_hash: BlockNumberAndHash,
    ) -> Option<packed::ProposalShortIdVec> {
        if let Some(cache) = self.cache() {
            if let Some(data) = cache.block_proposals.lock().get(&num_hash.hash()) {
                return Some(data.clone());
            }
        };

        let ret = self
            .get(
                COLUMN_BLOCK_PROPOSAL_IDS::NAME,
                COLUMN_BLOCK_PROPOSAL_IDS::key(num_hash.clone()).as_ref(),
            )
            .map(|slice| {
                packed::ProposalShortIdVecReader::from_slice_should_be_ok(slice.as_ref())
                    .to_entity()
            });

        if let Some(cache) = self.cache() {
            ret.map(|data| {
                cache
                    .block_proposals
                    .lock()
                    .put(num_hash.hash().clone(), data.clone());
                data
            })
        } else {
            ret
        }
    }

    /// Get block uncles by block header hash
    fn get_block_uncles(&self, num_hash: BlockNumberAndHash) -> Option<UncleBlockVecView> {
        if let Some(cache) = self.cache() {
            if let Some(data) = cache.block_uncles.lock().get(&num_hash.hash()) {
                return Some(data.clone());
            }
        };

        let ret = self
            .get(
                COLUMN_BLOCK_UNCLE::NAME,
                COLUMN_BLOCK_UNCLE::key(num_hash.clone()).as_ref(),
            )
            .map(|slice| {
                let reader =
                    packed::UncleBlockVecViewReader::from_slice_should_be_ok(slice.as_ref());
                Unpack::<UncleBlockVecView>::unpack(&reader)
            });

        if let Some(cache) = self.cache() {
            ret.map(|uncles| {
                cache.block_uncles.lock().put(num_hash.hash, uncles.clone());
                uncles
            })
        } else {
            ret
        }
    }

    /// Get block extension by block header hash
    fn get_block_extension(&self, hash: &packed::Byte32) -> Option<packed::Bytes> {
        if let Some(cache) = self.cache() {
            if let Some(data) = cache.block_extensions.lock().get(hash) {
                return data.clone();
            }
        };

        let ret = self
            .get(COLUMN_BLOCK_EXTENSION::NAME, hash.as_slice())
            .map(|slice| packed::BytesReader::from_slice_should_be_ok(slice.as_ref()).to_entity());

        if let Some(cache) = self.cache() {
            cache.block_extensions.lock().put(hash.clone(), ret.clone());
        }
        ret
    }

    /// Get block ext by block header hash
    ///
    /// Since v0.106, `BlockExt` added two option fields, so we have to use compatibility mode to read
    fn get_block_ext(&self, block_hash: &packed::Byte32) -> Option<BlockExt> {
        let block_number = self.get_block_number(block_hash)?;
        let num_hash = BlockNumberAndHash::new(block_number, block_hash.to_owned());
        self.get(
            COLUMN_BLOCK_EXT::NAME,
            COLUMN_BLOCK_EXT::key(num_hash).as_ref(),
        )
        .map(|slice| {
            let reader = packed::BlockExtReader::from_compatible_slice_should_be_ok(slice.as_ref());
            match reader.count_extra_fields() {
                0 => reader.unpack(),
                2 => packed::BlockExtV1Reader::from_slice_should_be_ok(slice.as_ref()).unpack(),
                _ => {
                    panic!(
                        "BlockExt storage field count doesn't match, expect 7 or 5, actual {}",
                        reader.field_count()
                    )
                }
            }
        })
    }

    /// Get block header hash by block number
    fn get_block_hash(&self, number: BlockNumber) -> Option<packed::Byte32> {
        let block_number: packed::Uint64 = number.pack();
        self.get(COLUMN_INDEX::NAME, block_number.as_slice())
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Get block number by block header hash
    fn get_block_number(&self, hash: &packed::Byte32) -> Option<BlockNumber> {
        self.get(COLUMN_BLOCK_HEADER_NUM::NAME, hash.as_slice())
            .map(|raw| packed::Uint64Reader::from_slice_should_be_ok(raw.as_ref()).unpack())
    }

    /// TODO(doc): @quake
    fn is_main_chain(&self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_INDEX::NAME, hash.as_slice()).is_some()
    }

    /// TODO(doc): @quake
    fn get_tip_header(&self) -> Option<HeaderView> {
        self.get(COLUMN_META::NAME, COLUMN_META::META_TIP_HEADER_KEY)
            .and_then(|raw| {
                self.get_block_header(
                    &packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity(),
                )
            })
            .map(Into::into)
    }

    /// Returns true if the transaction confirmed in main chain.
    ///
    /// This function is base on transaction index `COLUMN_TRANSACTION_INFO`.
    /// Current release maintains a full index of historical transaction by default, this may be changed in future
    fn transaction_exists(&self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_TRANSACTION_INFO::NAME, hash.as_slice())
            .is_some()
    }

    /// Get commit transaction and block hash by its hash
    fn get_transaction(&self, hash: &packed::Byte32) -> Option<(TransactionView, packed::Byte32)> {
        self.get_transaction_with_info(hash)
            .map(|(tx, tx_info)| (tx, tx_info.block_hash))
    }

    /// TODO(doc): @quake
    fn get_transaction_info(&self, hash: &packed::Byte32) -> Option<TransactionInfo> {
        self.get(COLUMN_TRANSACTION_INFO::NAME, hash.as_slice())
            .map(|slice| {
                let reader = packed::TransactionInfoReader::from_slice_should_be_ok(slice.as_ref());
                Unpack::<TransactionInfo>::unpack(&reader)
            })
    }

    fn get_transaction_block_number(&self, hash: &packed::Byte32) -> Option<BlockNumber> {
        self.get_transaction_info(hash)
            .map(|tx_info| tx_info.block_number)
    }

    /// Gets transaction and associated info with correspond hash
    fn get_transaction_with_info(
        &self,
        hash: &packed::Byte32,
    ) -> Option<(TransactionView, TransactionInfo)> {
        let tx_info = self.get_transaction_info(hash)?;
        if let Some(freezer) = self.freezer() {
            if tx_info.block_number > 0 && tx_info.block_number < freezer.number() {
                let raw_block = freezer
                    .retrieve(tx_info.block_number)
                    .expect("block frozen")?;
                let raw_block_reader =
                    packed::BlockReader::from_compatible_slice(&raw_block).expect("checked data");
                let tx_reader = raw_block_reader.transactions().get(tx_info.index)?;
                return Some((tx_reader.to_entity().into_view(), tx_info));
            }
        }
        self.get(COLUMN_BLOCK_BODY::NAME, tx_info.key().as_slice())
            .map(|slice| {
                let reader = packed::TransactionViewReader::from_slice_should_be_ok(slice.as_ref());
                (reader.unpack(), tx_info)
            })
    }

    /// Return whether cell is live
    fn have_cell(&self, out_point: &OutPoint) -> bool {
        if let Some(block_number) = self.get_transaction_block_number(&out_point.tx_hash()) {
            self.get(
                COLUMN_CELL::NAME,
                COLUMN_CELL::key(block_number, out_point).as_ref(),
            )
            .is_some()
        } else {
            false
        }
    }

    /// Gets cell meta data with out_point
    fn get_cell(&self, out_point: &OutPoint) -> Option<CellMeta> {
        let block_number = self.get_transaction_block_number(&out_point.tx_hash())?;
        self.get(
            COLUMN_CELL::NAME,
            COLUMN_CELL::key(block_number, out_point).as_ref(),
        )
        .map(|slice| {
            let reader = packed::CellEntryReader::from_slice_should_be_ok(slice.as_ref());
            build_cell_meta_from_reader(out_point.clone(), reader)
        })
    }

    /// TODO(doc): @quake
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<(Bytes, packed::Byte32)> {
        let block_number = self.get_transaction_block_number(&out_point.tx_hash())?;
        let key = COLUMN_CELL_DATA::key(block_number, out_point);
        if let Some(cache) = self.cache() {
            if let Some(cached) = cache.cell_data.lock().get(key.as_ref()) {
                return Some(cached.clone());
            }
        };

        let ret = self.get(COLUMN_CELL_DATA::NAME, key.as_ref()).map(|slice| {
            if !slice.as_ref().is_empty() {
                let reader = packed::CellDataEntryReader::from_slice_should_be_ok(slice.as_ref());
                let data = reader.output_data().unpack();
                let data_hash = reader.output_data_hash().to_entity();
                (data, data_hash)
            } else {
                // impl packed::CellOutput {
                //     pub fn calc_data_hash(data: &[u8]) -> packed::Byte32 {
                //         if data.is_empty() {
                //             packed::Byte32::zero()
                //         } else {
                //             blake2b_256(data).pack()
                //         }
                //     }
                // }
                (Bytes::new(), packed::Byte32::zero())
            }
        });

        if let Some(cache) = self.cache() {
            ret.map(|cached| {
                cache
                    .cell_data
                    .lock()
                    .put(key.as_ref().to_vec(), cached.clone());
                cached
            })
        } else {
            ret
        }
    }

    /// TODO(doc): @quake
    fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<packed::Byte32> {
        let block_number = self.get_transaction_block_number(&out_point.tx_hash())?;
        let key = COLUMN_CELL_DATA::key(block_number, out_point);
        if let Some(cache) = self.cache() {
            if let Some(cached) = cache.cell_data_hash.lock().get(key.as_ref()) {
                return Some(cached.clone());
            }
        };

        let ret = self
            .get(COLUMN_CELL_DATA_HASH::NAME, key.as_ref())
            .map(|raw| {
                if !raw.as_ref().is_empty() {
                    packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity()
                } else {
                    // impl packed::CellOutput {
                    //     pub fn calc_data_hash(data: &[u8]) -> packed::Byte32 {
                    //         if data.is_empty() {
                    //             packed::Byte32::zero()
                    //         } else {
                    //             blake2b_256(data).pack()
                    //         }
                    //     }
                    // }
                    packed::Byte32::zero()
                }
            });

        if let Some(cache) = self.cache() {
            ret.map(|cached| {
                cache
                    .cell_data_hash
                    .lock()
                    .put(key.as_ref().to_vec(), cached.clone());
                cached
            })
        } else {
            ret
        }
    }

    /// Gets current epoch ext
    fn get_current_epoch_ext(&self) -> Option<EpochExt> {
        self.get(COLUMN_META::NAME, COLUMN_META::META_CURRENT_EPOCH_KEY)
            .map(|slice| packed::EpochExtReader::from_slice_should_be_ok(slice.as_ref()).unpack())
    }

    /// Gets epoch ext by epoch index
    fn get_epoch_ext(&self, hash: &packed::Byte32) -> Option<EpochExt> {
        self.get(COLUMN_EPOCH::NAME, hash.as_slice())
            .map(|slice| packed::EpochExtReader::from_slice_should_be_ok(slice.as_ref()).unpack())
    }

    /// Gets epoch index by epoch number
    fn get_epoch_index(&self, number: EpochNumber) -> Option<packed::Byte32> {
        let epoch_number: packed::Uint64 = number.pack();
        self.get(COLUMN_EPOCH::NAME, epoch_number.as_slice())
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Gets epoch index by block hash
    fn get_block_epoch_index(&self, block_hash: &packed::Byte32) -> Option<packed::Byte32> {
        self.get(COLUMN_BLOCK_EPOCH::NAME, block_hash.as_slice())
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// TODO(doc): @quake
    fn get_block_epoch(&self, hash: &packed::Byte32) -> Option<EpochExt> {
        self.get_block_epoch_index(hash)
            .and_then(|index| self.get_epoch_ext(&index))
    }

    /// TODO(doc): @quake
    fn is_uncle(&self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_UNCLES::NAME, hash.as_slice()).is_some()
    }

    /// Gets header by uncle header hash
    fn get_uncle_header(&self, hash: &packed::Byte32) -> Option<HeaderView> {
        self.get(COLUMN_UNCLES::NAME, hash.as_slice()).map(|slice| {
            let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
            Unpack::<HeaderView>::unpack(&reader)
        })
    }

    /// TODO(doc): @quake
    fn block_exists(&self, hash: &packed::Byte32) -> bool {
        if let Some(cache) = self.cache() {
            if cache.headers.lock().get(hash).is_some() {
                return true;
            }
        };
        self.get(COLUMN_BLOCK_HEADER::NAME, hash.as_slice())
            .is_some()
    }

    /// Gets cellbase by block hash
    fn get_cellbase(&self, hash: &packed::Byte32) -> Option<TransactionView> {
        let number = self.get_block_number(hash).expect("block number");
        let num_hash = BlockNumberAndHash::new(number, hash.to_owned());

        let prefix = COLUMN_BLOCK_BODY::key(num_hash, 0);

        self.get(COLUMN_BLOCK_BODY::NAME, prefix.as_ref())
            .map(|slice| {
                let reader = packed::TransactionViewReader::from_slice_should_be_ok(slice.as_ref());
                Unpack::<TransactionView>::unpack(&reader)
            })
    }

    /// Gets latest built filter data block hash
    fn get_latest_built_filter_data_block_hash(&self) -> Option<packed::Byte32> {
        self.get(
            COLUMN_META::NAME,
            COLUMN_META::META_LATEST_BUILT_FILTER_DATA_KEY,
        )
        .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Gets block filter data by block hash
    fn get_block_filter(&self, num_hash: &BlockNumberAndHash) -> Option<packed::Bytes> {
        self.get(
            COLUMN_BLOCK_FILTER::NAME,
            COLUMN_BLOCK_FILTER::key(num_hash.to_owned()).as_ref(),
        )
        .map(|slice| packed::BytesReader::from_slice_should_be_ok(slice.as_ref()).to_entity())
    }

    /// Gets block filter hash by block hash
    fn get_block_filter_hash(&self, num_hash: BlockNumberAndHash) -> Option<packed::Byte32> {
        self.get(
            COLUMN_BLOCK_FILTER_HASH::NAME,
            COLUMN_BLOCK_FILTER_HASH::key(num_hash).as_ref(),
        )
        .map(|slice| packed::Byte32Reader::from_slice_should_be_ok(slice.as_ref()).to_entity())
    }

    /// Gets block bytes by block hash
    fn get_packed_block(&self, hash: &packed::Byte32) -> Option<packed::Block> {
        let header = self
            .get(COLUMN_BLOCK_HEADER::NAME, hash.as_slice())
            .map(|slice| {
                let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
                reader.data().to_entity()
            })?;

        let number: u64 = header.raw().number().unpack();
        let num_hash = BlockNumberAndHash::new(number, hash.to_owned());

        let prefix = COLUMN_BLOCK_BODY::prefix_key(num_hash.clone());

        let transactions: packed::TransactionVec = self
            .get_iter(
                COLUMN_BLOCK_BODY::NAME,
                IteratorMode::From(prefix.as_ref(), Direction::Forward),
            )
            .take_while(|(key, _)| key.starts_with(prefix.as_ref()))
            .map(|(_key, value)| {
                let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
                reader.data().to_entity()
            })
            .pack();

        let uncles = self.get_block_uncles(num_hash.clone())?;
        let proposals = self.get_block_proposal_txs_ids(num_hash)?;
        let extension_opt = self.get_block_extension(hash);

        let block = if let Some(extension) = extension_opt {
            packed::BlockV1::new_builder()
                .header(header)
                .uncles(uncles.data())
                .transactions(transactions)
                .proposals(proposals)
                .extension(extension)
                .build()
                .as_v0()
        } else {
            packed::Block::new_builder()
                .header(header)
                .uncles(uncles.data())
                .transactions(transactions)
                .proposals(proposals)
                .build()
        };

        Some(block)
    }

    /// Gets block header bytes by block hash
    fn get_packed_block_header(&self, hash: &packed::Byte32) -> Option<packed::Header> {
        let block_number: BlockNumber = self
            .get(COLUMN_BLOCK_HEADER_NUM::NAME, hash.as_slice())
            .map(|slice| packed::Uint64Reader::from_slice_should_be_ok(&slice).unpack())?;
        let num_hash = BlockNumberAndHash::new(block_number, hash.to_owned());
        self.get(
            COLUMN_BLOCK_HEADER::NAME,
            COLUMN_BLOCK_HEADER::key(num_hash).as_slice(),
        )
        .map(|slice| {
            let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
            reader.data().to_entity()
        })
    }

    /// Gets a header digest.
    fn get_header_digest(&self, position_u64: u64) -> Option<packed::HeaderDigest> {
        self.get(
            COLUMN_CHAIN_ROOT_MMR::NAME,
            COLUMN_CHAIN_ROOT_MMR::key(position_u64).as_slice(),
        )
        .map(|slice| {
            let reader = packed::HeaderDigestReader::from_slice_should_be_ok(slice.as_ref());
            reader.to_entity()
        })
    }

    /// Gets ancestor block header by a base block hash and number
    fn get_ancestor(&self, base: &packed::Byte32, number: BlockNumber) -> Option<HeaderView> {
        let header = self.get_block_header(base)?;
        if number > header.number() {
            None
        } else if number == header.number() {
            Some(header)
        } else if self.is_main_chain(base) {
            self.get_block_hash(number)
                .and_then(|hash| self.get_block_header(&hash))
        } else {
            self.get_ancestor(&header.parent_hash(), number)
        }
    }
}

fn build_cell_meta_from_reader(out_point: OutPoint, reader: packed::CellEntryReader) -> CellMeta {
    CellMeta {
        out_point,
        cell_output: reader.output().to_entity(),
        transaction_info: Some(TransactionInfo {
            block_number: reader.block_number().unpack(),
            block_hash: reader.block_hash().to_entity(),
            block_epoch: reader.block_epoch().unpack(),
            index: reader.index().unpack(),
        }),
        data_bytes: reader.data_size().unpack(),
        mem_cell_data: None,
        mem_cell_data_hash: None,
    }
}
