use crate::archive::ArchiveIndex;
use crate::cache::StoreCache;
use crate::data_loader_wrapper::BorrowedDataLoaderWrapper;
use ckb_db::{
    DBPinnableSlice,
    iter::{DBIter, Direction, IteratorMode},
};
use ckb_db_schema::{
    COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT,
    COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_FILTER, COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA,
    COLUMN_CELL_DATA_HASH, COLUMN_CHAIN_ROOT_MMR, COLUMN_EPOCH, COLUMN_INDEX, COLUMN_META,
    COLUMN_TRANSACTION_INFO, COLUMN_UNCLES, Col, META_CURRENT_EPOCH_KEY,
    META_LATEST_BUILT_FILTER_DATA_KEY, META_TIP_HEADER_KEY,
};
use ckb_freezer::{ArchivedBlock, Freezer};
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockExt, BlockNumber, BlockView, EpochExt, EpochNumber, HeaderView, TransactionInfo,
        TransactionView, UncleBlockVecView, cell::CellMeta,
    },
    packed::{self, OutPoint},
    prelude::*,
};

/// The `ChainStore` trait provides chain data store interface
pub trait ChainStore: Send + Sync + Sized {
    /// Return cache reference
    fn cache(&self) -> Option<&StoreCache>;
    /// Return freezer reference
    fn freezer(&self) -> Option<&Freezer>;
    /// Return the bytes associated with a key value and the given column family.
    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice<'_>>;
    /// Return an iterator over the database key-value pairs in the given column family.
    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter<'_>;
    /// Return the borrowed data loader wrapper
    fn borrow_as_data_loader(&self) -> BorrowedDataLoaderWrapper<'_, Self> {
        BorrowedDataLoaderWrapper::new(self)
    }

    /// Read an exact hash from the immutable archive using this store's DB view.
    fn get_archived_block(&self, hash: &packed::Byte32) -> Option<packed::Block> {
        match block_source(self, hash) {
            BlockSource::Archived { freezer, index } => {
                Some(read_archived_block(freezer, &index, hash))
            }
            BlockSource::Hot(_) => None,
        }
    }

    /// Read an archived view with the witness hashes committed in this DB view.
    fn get_archived_block_view(&self, hash: &packed::Byte32) -> Option<BlockView> {
        match block_source(self, hash) {
            BlockSource::Archived { freezer, index } => {
                let block = read_archived_block(freezer, &index, hash);
                Some(
                    index
                        .block_view(block)
                        .expect("valid archive witness hashes"),
                )
            }
            BlockSource::Hot(_) => None,
        }
    }

    /// Get block by block header hash
    fn get_block(&self, hash: &packed::Byte32) -> Option<BlockView> {
        let header = self.get(COLUMN_BLOCK_HEADER, hash.as_slice())?;
        if let Some(block) = self.get_archived_block_view(hash) {
            return Some(block);
        }
        let reader = packed::HeaderViewReader::from_slice_should_be_ok(header.as_ref());
        Some(read_hot_block(self, hash, reader.into()))
    }

    /// Get header by block header hash
    fn get_block_header(&self, hash: &packed::Byte32) -> Option<HeaderView> {
        if let Some(cache) = self.cache()
            && let Some(header) = cache.headers.lock().get(hash)
        {
            return Some(header.clone());
        };
        let ret = self.get(COLUMN_BLOCK_HEADER, hash.as_slice()).map(|slice| {
            let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
            Into::<HeaderView>::into(reader)
        });

        if let Some(cache) = self.cache() {
            ret.inspect(|header| {
                cache.headers.lock().put(hash.clone(), header.clone());
            })
        } else {
            ret
        }
    }

    /// Get block body by block header hash
    fn get_block_body(&self, hash: &packed::Byte32) -> Vec<TransactionView> {
        if let Some(block) = self.get_archived_block_view(hash) {
            return block.transactions();
        }
        self.get_hot_block_body(hash)
    }

    /// Read only the current KV view, including a transaction's own writes.
    fn get_hot_block_body(&self, hash: &packed::Byte32) -> Vec<TransactionView> {
        let prefix = hash.as_slice();
        let mut iter = self.get_iter(
            COLUMN_BLOCK_BODY,
            IteratorMode::From(prefix, Direction::Forward),
        );
        let mut body = Vec::new();
        for (key, value) in iter.by_ref() {
            if !key.starts_with(prefix) {
                break;
            }
            let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
            body.push(reader.into());
        }
        iter.status().expect("block body iteration failed");
        body
    }

    /// Get unfrozen block from ky-store with given hash
    fn get_unfrozen_block(&self, hash: &packed::Byte32) -> Option<BlockView> {
        let header = self
            .get(COLUMN_BLOCK_HEADER, hash.as_slice())
            .map(|slice| {
                let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
                Into::<HeaderView>::into(reader)
            })?;

        Some(read_hot_block(self, hash, header))
    }

    /// Get all transaction-hashes in block body by block header hash
    fn get_block_txs_hashes(&self, hash: &packed::Byte32) -> Vec<packed::Byte32> {
        match block_source(self, hash) {
            BlockSource::Archived { freezer, index } => {
                let block = read_archived_block(freezer, &index, hash);
                block
                    .transactions()
                    .into_iter()
                    .map(|tx| tx.calc_tx_hash())
                    .collect()
            }
            BlockSource::Hot(cache) => hot_block_txs_hashes(self, hash, cache),
        }
    }

    /// Get proposal short id by block header hash
    fn get_block_proposal_txs_ids(
        &self,
        hash: &packed::Byte32,
    ) -> Option<packed::ProposalShortIdVec> {
        match block_source(self, hash) {
            BlockSource::Archived { freezer, index } => Some(
                read_archived_fields(freezer, index.record(), hash)
                    .proposals()
                    .expect("valid archived proposals"),
            ),
            BlockSource::Hot(cache) => hot_block_proposals(self, hash, cache),
        }
    }

    /// Get block uncles by block header hash
    fn get_block_uncles(&self, hash: &packed::Byte32) -> Option<UncleBlockVecView> {
        match block_source(self, hash) {
            BlockSource::Archived { freezer, index } => Some(
                read_archived_fields(freezer, index.record(), hash)
                    .uncles()
                    .expect("valid archived uncles")
                    .into(),
            ),
            BlockSource::Hot(cache) => hot_block_uncles(self, hash, cache),
        }
    }

    /// Get block extension by block header hash
    fn get_block_extension(&self, hash: &packed::Byte32) -> Option<packed::Bytes> {
        match block_source(self, hash) {
            BlockSource::Archived { freezer, index } => {
                read_archived_fields(freezer, index.record(), hash)
                    .extension()
                    .expect("archive extension read failed")
            }
            BlockSource::Hot(cache) => hot_block_extension(self, hash, cache),
        }
    }

    /// Get block ext by block header hash
    ///
    /// Since v0.106, `BlockExt` added two option fields, so we have to use compatibility mode to read
    fn get_block_ext(&self, block_hash: &packed::Byte32) -> Option<BlockExt> {
        self.get(COLUMN_BLOCK_EXT, block_hash.as_slice())
            .map(|slice| {
                let reader =
                    packed::BlockExtReader::from_compatible_slice_should_be_ok(slice.as_ref());
                match reader.count_extra_fields() {
                    0 => reader.into(),
                    2 => packed::BlockExtV1Reader::from_slice_should_be_ok(slice.as_ref()).into(),
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
        let block_number: packed::Uint64 = number.into();
        self.get(COLUMN_INDEX, block_number.as_slice())
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Get block number by block header hash
    fn get_block_number(&self, hash: &packed::Byte32) -> Option<BlockNumber> {
        self.get(COLUMN_INDEX, hash.as_slice())
            .map(|raw| packed::Uint64Reader::from_slice_should_be_ok(raw.as_ref()).into())
    }

    /// Returns true if the block is on the main chain.
    fn is_main_chain(&self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_INDEX, hash.as_slice()).is_some()
    }

    /// Returns the header of the chain tip.
    fn get_tip_header(&self) -> Option<HeaderView> {
        self.get(COLUMN_META, META_TIP_HEADER_KEY).and_then(|raw| {
            self.get_block_header(
                &packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity(),
            )
        })
    }

    /// Returns true if the transaction confirmed in main chain.
    ///
    /// This function is base on transaction index `COLUMN_TRANSACTION_INFO`.
    /// Current release maintains a full index of historical transaction by default, this may be changed in future
    fn transaction_exists(&self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_TRANSACTION_INFO, hash.as_slice()).is_some()
    }

    /// Get commit transaction and block hash by its hash
    fn get_transaction(&self, hash: &packed::Byte32) -> Option<(TransactionView, packed::Byte32)> {
        self.get_transaction_with_info(hash)
            .map(|(tx, tx_info)| (tx, tx_info.block_hash))
    }

    /// Returns transaction info by transaction hash.
    fn get_transaction_info(&self, hash: &packed::Byte32) -> Option<TransactionInfo> {
        self.get(COLUMN_TRANSACTION_INFO, hash.as_slice())
            .map(|slice| {
                let reader = packed::TransactionInfoReader::from_slice_should_be_ok(slice.as_ref());
                Into::<TransactionInfo>::into(reader)
            })
    }

    /// Gets transaction and associated info with correspond hash
    fn get_transaction_with_info(
        &self,
        hash: &packed::Byte32,
    ) -> Option<(TransactionView, TransactionInfo)> {
        let tx_info = self.get_transaction_info(hash)?;
        if let BlockSource::Archived { freezer, index } = block_source(self, &tx_info.block_hash) {
            let fields = read_archived_fields(freezer, index.record(), &tx_info.block_hash);
            let tx = index
                .transaction(&fields, tx_info.index)
                .expect("valid archived transaction")
                .expect("archive transaction index out of bounds");
            assert_eq!(tx.hash(), *hash, "archive transaction hash mismatch");
            return Some((tx, tx_info));
        }
        self.get(COLUMN_BLOCK_BODY, tx_info.key().as_slice())
            .map(|slice| {
                let reader = packed::TransactionViewReader::from_slice_should_be_ok(slice.as_ref());
                (reader.into(), tx_info)
            })
    }

    /// Return whether cell is live
    fn have_cell(&self, out_point: &OutPoint) -> bool {
        let key = out_point.to_cell_key();
        self.get(COLUMN_CELL, &key).is_some()
    }

    /// Gets cell meta data with out_point
    fn get_cell(&self, out_point: &OutPoint) -> Option<CellMeta> {
        let key = out_point.to_cell_key();
        self.get(COLUMN_CELL, &key).map(|slice| {
            let reader = packed::CellEntryReader::from_slice_should_be_ok(slice.as_ref());
            build_cell_meta_from_reader(out_point.clone(), reader)
        })
    }

    /// Returns cell data and its hash for the given outpoint.
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<(Bytes, packed::Byte32)> {
        let key = out_point.to_cell_key();
        if let Some(cache) = self.cache()
            && let Some(cached) = cache.cell_data.lock().get(&key)
        {
            return Some(cached.clone());
        };

        let ret = self.get(COLUMN_CELL_DATA, &key).map(|slice| {
            if !slice.as_ref().is_empty() {
                let reader = packed::CellDataEntryReader::from_slice_should_be_ok(slice.as_ref());
                let data = reader.output_data().into();
                let data_hash = reader.output_data_hash().to_entity();
                (data, data_hash)
            } else {
                (Bytes::new(), packed::Byte32::zero())
            }
        });

        if let Some(cache) = self.cache() {
            ret.inspect(|cached| {
                cache.cell_data.lock().put(key, cached.clone());
            })
        } else {
            ret
        }
    }

    /// Returns the hash of cell data for the given outpoint.
    fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<packed::Byte32> {
        let key = out_point.to_cell_key();
        if let Some(cache) = self.cache()
            && let Some(cached) = cache.cell_data_hash.lock().get(&key)
        {
            return Some(cached.clone());
        };

        let ret = self.get(COLUMN_CELL_DATA_HASH, &key).map(|raw| {
            if !raw.as_ref().is_empty() {
                packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity()
            } else {
                packed::Byte32::zero()
            }
        });

        if let Some(cache) = self.cache() {
            ret.inspect(|cached| {
                cache.cell_data_hash.lock().put(key, cached.clone());
            })
        } else {
            ret
        }
    }

    /// Gets current epoch ext
    fn get_current_epoch_ext(&self) -> Option<EpochExt> {
        self.get(COLUMN_META, META_CURRENT_EPOCH_KEY)
            .map(|slice| packed::EpochExtReader::from_slice_should_be_ok(slice.as_ref()).into())
    }

    /// Gets epoch ext by epoch index
    fn get_epoch_ext(&self, hash: &packed::Byte32) -> Option<EpochExt> {
        self.get(COLUMN_EPOCH, hash.as_slice())
            .map(|slice| packed::EpochExtReader::from_slice_should_be_ok(slice.as_ref()).into())
    }

    /// Gets epoch index by epoch number
    fn get_epoch_index(&self, number: EpochNumber) -> Option<packed::Byte32> {
        let epoch_number: packed::Uint64 = number.into();
        self.get(COLUMN_EPOCH, epoch_number.as_slice())
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Gets epoch index by block hash
    fn get_block_epoch_index(&self, block_hash: &packed::Byte32) -> Option<packed::Byte32> {
        self.get(COLUMN_BLOCK_EPOCH, block_hash.as_slice())
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Returns the epoch of the block with the given hash.
    fn get_block_epoch(&self, hash: &packed::Byte32) -> Option<EpochExt> {
        self.get_block_epoch_index(hash)
            .and_then(|index| self.get_epoch_ext(&index))
    }

    /// Returns true if the given hash is an uncle block.
    fn is_uncle(&self, hash: &packed::Byte32) -> bool {
        self.get(COLUMN_UNCLES, hash.as_slice()).is_some()
    }

    /// Gets header by uncle header hash
    fn get_uncle_header(&self, hash: &packed::Byte32) -> Option<HeaderView> {
        self.get(COLUMN_UNCLES, hash.as_slice()).map(|slice| {
            let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
            Into::<HeaderView>::into(reader)
        })
    }

    /// Returns true if a block with the given hash exists in the store.
    fn block_exists(&self, hash: &packed::Byte32) -> bool {
        if let Some(cache) = self.cache()
            && cache.headers.lock().get(hash).is_some()
        {
            return true;
        };
        self.get(COLUMN_BLOCK_HEADER, hash.as_slice()).is_some()
    }

    /// Gets cellbase by block hash
    fn get_cellbase(&self, hash: &packed::Byte32) -> Option<TransactionView> {
        if let BlockSource::Archived { freezer, index } = block_source(self, hash) {
            let fields = read_archived_fields(freezer, index.record(), hash);
            return index
                .transaction(&fields, 0)
                .expect("valid archived cellbase");
        }

        let key = packed::TransactionKey::new_builder()
            .block_hash(hash.to_owned())
            .build();
        self.get(COLUMN_BLOCK_BODY, key.as_slice()).map(|slice| {
            let reader = packed::TransactionViewReader::from_slice_should_be_ok(slice.as_ref());
            Into::<TransactionView>::into(reader)
        })
    }

    /// Gets latest built filter data block hash
    fn get_latest_built_filter_data_block_hash(&self) -> Option<packed::Byte32> {
        self.get(COLUMN_META, META_LATEST_BUILT_FILTER_DATA_KEY)
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Gets block filter data by block hash
    fn get_block_filter(&self, hash: &packed::Byte32) -> Option<packed::Bytes> {
        self.get(COLUMN_BLOCK_FILTER, hash.as_slice())
            .map(|slice| packed::BytesReader::from_slice_should_be_ok(slice.as_ref()).to_entity())
    }

    /// Gets block filter hash by block hash
    fn get_block_filter_hash(&self, hash: &packed::Byte32) -> Option<packed::Byte32> {
        self.get(COLUMN_BLOCK_FILTER_HASH, hash.as_slice())
            .map(|slice| packed::Byte32Reader::from_slice_should_be_ok(slice.as_ref()).to_entity())
    }

    /// Gets block bytes by block hash
    fn get_packed_block(&self, hash: &packed::Byte32) -> Option<packed::Block> {
        let cache = match block_source(self, hash) {
            BlockSource::Archived { freezer, index } => {
                let block = read_archived_block(freezer, &index, hash);
                self.get(COLUMN_BLOCK_HEADER, hash.as_slice())?;
                return Some(block);
            }
            BlockSource::Hot(cache) => cache,
        };

        let header = self
            .get(COLUMN_BLOCK_HEADER, hash.as_slice())
            .map(|slice| {
                let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
                reader.data().to_entity()
            })?;

        let prefix = hash.as_slice();
        let mut iter = self.get_iter(
            COLUMN_BLOCK_BODY,
            IteratorMode::From(prefix, Direction::Forward),
        );
        let transactions: packed::TransactionVec = iter
            .by_ref()
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(_key, value)| {
                let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
                reader.data().to_entity()
            })
            .collect::<Vec<_>>()
            .into();
        iter.status()
            .expect("packed block transaction iteration failed");

        let uncles = hot_block_uncles(self, hash, cache)?;
        let proposals = hot_block_proposals(self, hash, cache)?;
        let extension_opt = hot_block_extension(self, hash, cache);

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
        self.get(COLUMN_BLOCK_HEADER, hash.as_slice()).map(|slice| {
            let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
            reader.data().to_entity()
        })
    }

    /// Gets a header digest.
    fn get_header_digest(&self, position_u64: u64) -> Option<packed::HeaderDigest> {
        let position: packed::Uint64 = position_u64.into();
        self.get(COLUMN_CHAIN_ROOT_MMR, position.as_slice())
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
            block_number: reader.block_number().into(),
            block_hash: reader.block_hash().to_entity(),
            block_epoch: reader.block_epoch().into(),
            index: reader.index().into(),
        }),
        data_bytes: reader.data_size().into(),
        mem_cell_data: None,
        mem_cell_data_hash: None,
    }
}

/// A hot override also disables immutable payload caches in this read view.
enum BlockSource<'a> {
    Archived {
        freezer: &'a Freezer,
        index: ArchiveIndex<'a>,
    },
    Hot(Option<&'a StoreCache>),
}

fn block_source<'a>(store: &'a impl ChainStore, hash: &packed::Byte32) -> BlockSource<'a> {
    let Some(freezer) = store.freezer() else {
        return BlockSource::Hot(store.cache());
    };
    let Some(raw) = store.get(COLUMN_BLOCK_ARCHIVE, hash.as_slice()) else {
        return BlockSource::Hot(store.cache());
    };
    match ArchiveIndex::new(raw).expect("valid archive mapping") {
        None => BlockSource::Hot(None),
        Some(index) => BlockSource::Archived { freezer, index },
    }
}

fn read_archived_block(
    freezer: &Freezer,
    index: &ArchiveIndex<'_>,
    hash: &packed::Byte32,
) -> packed::Block {
    let bytes = freezer
        .retrieve(index.record())
        .expect("archive read failed")
        .expect("committed archive record is missing");
    let block = packed::BlockReader::from_compatible_slice(&bytes).expect("valid archived block");
    assert!(
        block.count_extra_fields() <= 1,
        "unsupported archived block fields"
    );
    assert_eq!(
        block.calc_header_hash(),
        *hash,
        "archive mapping hash mismatch"
    );
    index
        .check_count(block.transactions().len())
        .expect("valid archive witness hashes");
    packed::Block::new_unchecked(bytes.into())
}

fn read_archived_fields<'a>(
    freezer: &'a Freezer,
    record: u64,
    hash: &packed::Byte32,
) -> ArchivedBlock<'a> {
    freezer
        .read_block(record, hash)
        .expect("archive block layout read failed")
        .expect("committed archive record is missing")
}

fn hot_block_txs_hashes(
    store: &impl ChainStore,
    hash: &packed::Byte32,
    cache: Option<&StoreCache>,
) -> Vec<packed::Byte32> {
    if let Some(cache) = cache
        && let Some(hashes) = cache.block_tx_hashes.lock().get(hash)
    {
        return hashes.clone();
    };

    let prefix = hash.as_slice();
    let mut iter = store.get_iter(
        COLUMN_BLOCK_BODY,
        IteratorMode::From(prefix, Direction::Forward),
    );
    let ret: Vec<_> = iter
        .by_ref()
        .take_while(|(key, _)| key.starts_with(prefix))
        .map(|(_key, value)| {
            let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
            reader.hash().to_entity()
        })
        .collect();
    iter.status()
        .expect("block transaction hash iteration failed");

    if let Some(cache) = cache {
        cache.block_tx_hashes.lock().put(hash.clone(), ret.clone());
    }

    ret
}

fn hot_block_proposals(
    store: &impl ChainStore,
    hash: &packed::Byte32,
    cache: Option<&StoreCache>,
) -> Option<packed::ProposalShortIdVec> {
    if let Some(cache) = cache
        && let Some(data) = cache.block_proposals.lock().get(hash)
    {
        return Some(data.clone());
    };

    let ret = store
        .get(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())
        .map(|slice| {
            packed::ProposalShortIdVecReader::from_slice_should_be_ok(slice.as_ref()).to_entity()
        });

    if let Some(cache) = cache {
        ret.inspect(|data| {
            cache.block_proposals.lock().put(hash.clone(), data.clone());
        })
    } else {
        ret
    }
}

fn hot_block_uncles(
    store: &impl ChainStore,
    hash: &packed::Byte32,
    cache: Option<&StoreCache>,
) -> Option<UncleBlockVecView> {
    if let Some(cache) = cache
        && let Some(data) = cache.block_uncles.lock().get(hash)
    {
        return Some(data.clone());
    };

    let ret = store.get(COLUMN_BLOCK_UNCLE, hash.as_slice()).map(|slice| {
        let reader = packed::UncleBlockVecViewReader::from_slice_should_be_ok(slice.as_ref());
        Into::<UncleBlockVecView>::into(reader)
    });

    if let Some(cache) = cache {
        ret.inspect(|uncles| {
            cache.block_uncles.lock().put(hash.clone(), uncles.clone());
        })
    } else {
        ret
    }
}

fn hot_block_extension(
    store: &impl ChainStore,
    hash: &packed::Byte32,
    cache: Option<&StoreCache>,
) -> Option<packed::Bytes> {
    if let Some(cache) = cache
        && let Some(data) = cache.block_extensions.lock().get(hash)
    {
        return data.clone();
    };

    let ret = store
        .get(COLUMN_BLOCK_EXTENSION, hash.as_slice())
        .map(|slice| packed::BytesReader::from_slice_should_be_ok(slice.as_ref()).to_entity());

    if let Some(cache) = cache {
        cache.block_extensions.lock().put(hash.clone(), ret.clone());
    }
    ret
}

fn read_hot_block(store: &impl ChainStore, hash: &packed::Byte32, header: HeaderView) -> BlockView {
    let body = store.get_hot_block_body(hash);

    let uncles = hot_block_uncles(store, hash, None).expect("block uncles must be stored");
    let proposals =
        hot_block_proposals(store, hash, None).expect("block proposal_ids must be stored");
    let extension_opt = hot_block_extension(store, hash, None);

    if let Some(extension) = extension_opt {
        BlockView::new_unchecked_with_extension(header, uncles, body, proposals, extension)
    } else {
        BlockView::new_unchecked(header, uncles, body, proposals)
    }
}
