use crate::{ChainDB, ChainStore};
use ckb_db::{DBIterator, DBPinnableSlice};
use ckb_db_schema::{
    COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_META, COLUMN_NUMBER_HASH, Col,
    META_ARCHIVE_NEXT_RECORD, META_ARCHIVE_TIP,
};
use ckb_error::{Error, InternalErrorKind};
use ckb_types::{
    core::{BlockView, TransactionView},
    packed,
    prelude::*,
};

fn invalid(reason: impl std::fmt::Display) -> Error {
    InternalErrorKind::DataCorrupted
        .other(reason.to_string())
        .into()
}

pub(crate) fn record_number(raw: &[u8]) -> Result<u64, Error> {
    let number = u64::from_le_bytes(raw.try_into().map_err(invalid)?);
    if number == 0 {
        return Err(invalid("zero archive record number"));
    }
    Ok(number)
}

/// The record and cached witness hashes belong to the same DB read view.
/// Recovery publishes them only after matching every hot payload byte.
pub(crate) struct ArchiveIndex<'a>(DBPinnableSlice<'a>);

impl<'a> ArchiveIndex<'a> {
    pub(crate) fn new(raw: DBPinnableSlice<'a>) -> Result<Option<Self>, Error> {
        if raw.as_ref() == 0u64.to_le_bytes() {
            return Ok(None);
        }
        if raw.len() < 8 || !(raw.len() - 8).is_multiple_of(32) {
            return Err(invalid("invalid archive mapping length"));
        }
        record_number(&raw[..8])?;
        Ok(Some(Self(raw)))
    }

    pub(crate) fn record(&self) -> u64 {
        u64::from_le_bytes(self.0[..8].try_into().expect("validated archive index"))
    }

    pub(crate) fn check_count(&self, count: usize) -> Result<(), Error> {
        if count != (self.0.len() - 8) / 32 {
            return Err(invalid("archive transaction count mismatch"));
        }
        Ok(())
    }

    pub(crate) fn block_view(&self, block: packed::Block) -> Result<BlockView, Error> {
        self.check_count(block.transactions().len())?;
        let hashes = block.calc_tx_hashes();
        let witness_hashes = self.0[8..]
            .chunks_exact(32)
            .map(|raw| packed::Byte32::from_slice(raw).expect("hash size"))
            .collect();
        Ok(packed::Block::block_into_view_internal(
            block,
            hashes,
            witness_hashes,
        ))
    }

    pub(crate) fn transaction(
        &self,
        block: &ckb_freezer::ArchivedBlock<'_>,
        index: usize,
    ) -> Result<Option<TransactionView>, Error> {
        let (transaction, count) = block.transaction_with_count(index)?;
        self.check_count(count)?;
        transaction
            .map(|transaction| {
                let raw = self.0[8..]
                    .chunks_exact(32)
                    .nth(index)
                    .ok_or_else(|| invalid("archive transaction index out of bounds"))?;
                let hash = transaction.calc_tx_hash();
                let witness_hash = packed::Byte32::from_slice(raw).expect("hash size");
                Ok(TransactionView::new_unchecked(
                    transaction,
                    hash,
                    witness_hash,
                ))
            })
            .transpose()
    }
}

pub(crate) fn block_hash(col: Col, key: &[u8]) -> Option<&[u8; 32]> {
    let bytes = match col {
        COLUMN_BLOCK_BODY if key.len() == 36 => &key[..32],
        COLUMN_BLOCK_UNCLE | COLUMN_BLOCK_PROPOSAL_IDS | COLUMN_BLOCK_EXTENSION
            if key.len() == 32 =>
        {
            key
        }
        COLUMN_NUMBER_HASH if key.len() == 40 => &key[8..],
        _ => return None,
    };
    bytes.try_into().ok()
}

/// A decision for the collector's one source snapshot applies to every payload
/// key of this block. Later writes are reconciled separately by the collector.
fn retain_payload(
    snapshot: &ckb_db::RocksDBSnapshot,
    freezer: &ckb_freezer::Freezer,
    hash: &[u8; 32],
) -> Result<bool, Error> {
    let Some(raw) = snapshot.get_pinned(COLUMN_BLOCK_ARCHIVE, hash)? else {
        return Ok(true);
    };
    let Some(index) = ArchiveIndex::new(raw)? else {
        return Ok(true);
    };
    if snapshot.get_pinned(COLUMN_BLOCK_HEADER, hash)?.is_none() {
        return Ok(true);
    }
    let Some(ext) = snapshot.get_pinned(ckb_db_schema::COLUMN_BLOCK_EXT, hash)? else {
        return Ok(true);
    };
    let ext = packed::BlockExtReader::from_compatible_slice(&ext).map_err(invalid)?;
    if ext.verified().to_opt().map(|value| value.into()) != Some(true) {
        return Ok(true);
    }
    let bytes = freezer
        .retrieve_limited(
            index.record(),
            ckb_freezer::FreezerServiceConfig::default().io_bytes as usize,
        )?
        .ok_or_else(|| invalid("indexed archive record is missing"))?;
    let block = packed::BlockReader::from_compatible_slice(&bytes).map_err(invalid)?;
    if block.count_extra_fields() > 1 || block.calc_header_hash().as_slice() != hash {
        return Err(invalid("archive record does not match its indexed block"));
    }
    index.check_count(block.transactions().len())?;
    Ok(false)
}

/// Emit the existing logical payload encodings when a caller edits a cold block.
/// The archive mapping is replaced with a hot override in the same transaction/batch, so ordinary
/// reads, deletes and savepoint rollback all use the database's own semantics.
pub(crate) fn restore_payload<E>(
    block: &BlockView,
    mut put: impl FnMut(Col, &[u8], &[u8]) -> Result<(), E>,
) -> Result<(), E> {
    let hash = block.hash();
    let uncles: packed::UncleBlockVecView = block.uncles().into();
    put(COLUMN_BLOCK_UNCLE, hash.as_slice(), uncles.as_slice())?;
    put(
        COLUMN_BLOCK_PROPOSAL_IDS,
        hash.as_slice(),
        block.data().proposals().as_slice(),
    )?;
    if let Some(extension) = block.extension() {
        put(
            COLUMN_BLOCK_EXTENSION,
            hash.as_slice(),
            extension.as_slice(),
        )?;
    }
    let number_hash = packed::NumberHash::new_builder()
        .number(block.number())
        .block_hash(hash.clone())
        .build();
    let count: packed::Uint32 = (block.transactions().len() as u32).into();
    put(COLUMN_NUMBER_HASH, number_hash.as_slice(), count.as_slice())?;
    for (index, tx) in block.transactions().into_iter().enumerate() {
        let key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(index)
            .build();
        let value: packed::TransactionView = tx.into();
        put(COLUMN_BLOCK_BODY, key.as_slice(), value.as_slice())?;
    }
    Ok(())
}

/// None means the pause budget expired; false means the immutable copy differs.
fn payload_matches(
    snapshot: &ckb_db::RocksDBSnapshot,
    block: &BlockView,
    should_stop: impl Fn() -> bool,
) -> Result<Option<bool>, Error> {
    enum Check {
        Deferred,
        Different,
        Read(Error),
    }
    if should_stop() {
        return Ok(None);
    }
    let hash = block.hash();
    // Existing mappings, including a hot override, must never be republished.
    if snapshot
        .get_pinned(COLUMN_BLOCK_ARCHIVE, hash.as_slice())?
        .is_some()
    {
        return Ok(Some(false));
    }
    let header: packed::HeaderView = block.header().into();
    if snapshot
        .get_pinned(COLUMN_BLOCK_HEADER, hash.as_slice())?
        .is_none_or(|raw| &*raw != header.as_slice())
    {
        return Ok(Some(false));
    }
    let Some(ext) = snapshot.get_pinned(ckb_db_schema::COLUMN_BLOCK_EXT, hash.as_slice())? else {
        return Ok(Some(false));
    };
    let ext = packed::BlockExtReader::from_compatible_slice(&ext).map_err(invalid)?;
    if ext.verified().to_opt().map(|value| value.into()) != Some(true) {
        return Ok(Some(false));
    }
    match restore_payload(block, |col, key, value| {
        if should_stop() {
            return Err(Check::Deferred);
        }
        if snapshot
            .get_pinned(col, key)
            .map_err(Check::Read)?
            .is_none_or(|raw| &*raw != value)
        {
            return Err(Check::Different);
        }
        Ok(())
    }) {
        Err(Check::Deferred) => return Ok(None),
        Err(Check::Different) => return Ok(Some(false)),
        Err(Check::Read(error)) => return Err(error),
        Ok(()) => {}
    }
    if should_stop() {
        return Ok(None);
    }
    if block.extension().is_none()
        && snapshot
            .get_pinned(COLUMN_BLOCK_EXTENSION, hash.as_slice())?
            .is_some()
    {
        return Ok(Some(false));
    }
    let expected = block.transactions().len();
    let mut iter = snapshot.iter(
        COLUMN_BLOCK_BODY,
        ckb_db::IteratorMode::From(hash.as_slice(), ckb_db::Direction::Forward),
    )?;
    let mut count = 0;
    // One excess key proves a mismatch; never scan an arbitrary corrupt prefix.
    for (key, _) in iter.by_ref() {
        if should_stop() {
            return Ok(None);
        }
        if !key.starts_with(hash.as_slice()) {
            break;
        }
        count += 1;
        if count > expected {
            break;
        }
    }
    iter.status().map_err(invalid)?;
    Ok(Some(count == expected))
}

impl ChainDB {
    /// Resolve committed file records left before their database index commit.
    /// Each bounded batch is synchronized before advancing the recovery cursor.
    /// Files are immutable; records for deleted or unverified blocks stay unindexed.
    pub fn recover_archive(&self) -> Result<u64, Error> {
        self.recover_archive_with_cancel(|| false)
    }

    /// Cancel between records/batches, leaving a durable cursor for the next run.
    pub fn recover_archive_with_cancel(
        &self,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<u64, Error> {
        let _writer = self.archive_writer.lock();
        let indexed = self
            .db()
            .get_pinned(COLUMN_META, META_ARCHIVE_NEXT_RECORD)?
            .map(|raw| record_number(&raw))
            .transpose()?;
        let Some(freezer) = self.freezer() else {
            return if indexed.is_none() {
                Ok(1)
            } else {
                Err(invalid("database requires its archive directory"))
            };
        };
        let next = indexed.unwrap_or(1);
        let end = freezer.number();
        if next > end {
            return Err(invalid(
                "archive is shorter than its committed database index",
            ));
        }
        if indexed.is_none() {
            // Opening the archive has durably committed its empty prefix. Its
            // first recovery cursor also makes activation permanent in the DB.
            let mut batch = self.new_write_batch();
            batch.put(COLUMN_META, META_ARCHIVE_NEXT_RECORD, &next.to_le_bytes())?;
            self.write_sync(&batch)?;
        }
        let mut start = next;
        while start < end {
            // 16 MiB is the batching target. Aggregate raw+compressed buffer
            // and both encoded index copies share the node archiver's budget.
            // BlockView hashes and container metadata require additional memory.
            let mut records = Vec::new();
            let mut bytes = 0;
            let mut read_bytes = 0;
            let read_limit = ckb_freezer::FreezerServiceConfig::default().io_bytes as usize;
            let mut stop = start;
            while stop < end && records.len() < 128 && bytes < 16 << 20 {
                if is_cancelled() {
                    return Ok(start);
                }
                let record = freezer
                    .record(stop)?
                    .ok_or_else(|| invalid("committed archive record is missing"))?;
                let required = record.read_bytes();
                if required > read_limit {
                    return Err(invalid(
                        "archive record exceeds the node archive I/O budget",
                    ));
                }
                if required > read_limit - read_bytes {
                    break;
                }
                let raw = record.retrieve_limited(read_limit - read_bytes)?;
                let block = packed::BlockReader::from_compatible_slice(&raw).map_err(invalid)?;
                if block.count_extra_fields() > 1 {
                    return Err(invalid("unsupported archived block fields"));
                }
                let index_bytes = block
                    .transactions()
                    .len()
                    .checked_mul(32)
                    .and_then(|len| len.checked_add(8))
                    .ok_or_else(|| invalid("archive index size overflow"))?;
                let required = index_bytes
                    .checked_mul(2)
                    .and_then(|len| len.checked_add(required))
                    .ok_or_else(|| invalid("archive buffer size overflow"))?;
                if required > read_limit {
                    return Err(invalid(
                        "archive record and index exceed the node archive I/O budget",
                    ));
                }
                if required > read_limit - read_bytes {
                    break;
                }
                bytes += raw.len();
                read_bytes += required;
                let block =
                    packed::Block::new_unchecked(raw.into()).into_view_without_reset_header();
                let mut index = Vec::with_capacity(index_bytes);
                index.extend_from_slice(&stop.to_le_bytes());
                for hash in block.tx_witness_hashes() {
                    index.extend_from_slice(hash.as_slice());
                }
                records.push((stop, block, index));
                stop += 1;
            }
            if is_cancelled() {
                return Ok(start);
            }
            fail::fail_point!("freezer-index-before-publish", |_| Err(invalid(
                "injected archive index failure before publication"
            )));
            let mut processed = start;
            let published = self.db().write_when_idle(
                std::time::Duration::from_millis(100),
                |snapshot, batch| {
                    let started = std::time::Instant::now();
                    fail::fail_point!("freezer-index-paused");
                    let should_stop = || {
                        is_cancelled() || started.elapsed() >= std::time::Duration::from_millis(100)
                    };
                    for (record, block, index) in &records {
                        // A stale source snapshot may have been edited before the file
                        // was committed. Only exact, still-present payload is publishable.
                        match payload_matches(snapshot, block, should_stop)? {
                            Some(true) => {
                                batch.put(COLUMN_BLOCK_ARCHIVE, block.hash().as_slice(), index)?
                            }
                            Some(false) => {}
                            None => break,
                        }
                        processed = record + 1;
                        fail::fail_point!("freezer-index-after-record");
                    }
                    if processed == start {
                        return Ok(false);
                    }
                    batch.put(
                        COLUMN_META,
                        META_ARCHIVE_NEXT_RECORD,
                        &processed.to_le_bytes(),
                    )?;
                    Ok(true)
                },
            )?;
            if !published {
                return Ok(start);
            }
            fail::fail_point!("freezer-index-after-publish", |_| Err(invalid(
                "injected archive index failure after publication"
            )));
            start = processed;
        }
        Ok(end)
    }

    /// Record progress by hash as well as height; a later reorg can find its fork.
    pub fn set_archive_tip(&self, number: u64, hash: packed::Byte32) -> Result<(), Error> {
        let tip = packed::NumberHash::new_builder()
            .number(number)
            .block_hash(hash)
            .build();
        let mut batch = self.new_write_batch();
        batch.put(COLUMN_META, META_ARCHIVE_TIP, tip.as_slice())?;
        self.write_sync(&batch)
    }

    /// Reclaim a physical generation only when enough archived blocks accumulated
    /// to amortize copying the retained hot data. The estimate affects scheduling,
    /// never the membership proof used by the collector.
    pub fn archive_collection_due(&self) -> Result<bool, Error> {
        let next = self
            .db()
            .get_pinned(COLUMN_META, META_ARCHIVE_NEXT_RECORD)?
            .map(|raw| record_number(&raw))
            .transpose()?
            .unwrap_or(1);
        let collected = self
            .db()
            .get_pinned(COLUMN_META, ckb_db_schema::META_ARCHIVE_COLLECTED)?
            .map(|raw| record_number(&raw))
            .transpose()?
            .unwrap_or(1);
        let pending = next
            .checked_sub(collected)
            .ok_or_else(|| invalid("archive collection frontier exceeds its index"))?;
        let blocks = self
            .db()
            .estimate_num_keys_cf(COLUMN_NUMBER_HASH)?
            .unwrap_or(0);
        Ok(pending >= blocks.div_ceil(2).max(1024))
    }

    /// Copy hot payloads, publish their replacement CFs, and retire the old CFs.
    /// Every omitted block is indexed by the copy snapshot and its immutable
    /// archive record is read and checked before the healthy hot copy is removed.
    pub fn collect_archive(
        &self,
        options: &ckb_db::CollectionOptions,
        after_publish: impl FnOnce(),
    ) -> Result<ckb_db::CollectionStats, Error> {
        self.collect_archive_with_cancel(options, || false, after_publish)
    }

    /// Stop copying when requested, while completing any begun publication.
    pub fn collect_archive_with_cancel(
        &self,
        options: &ckb_db::CollectionOptions,
        is_cancelled: impl Fn() -> bool,
        after_publish: impl FnOnce(),
    ) -> Result<ckb_db::CollectionStats, Error> {
        let freezer = self
            .freezer()
            .ok_or_else(|| invalid("archive is not open"))?;
        let _writer = self.archive_writer.lock();
        let mut retained = lru::LruCache::new(128);
        self.db().collect_payload_with_cancel(
            options,
            |snapshot, col, key, _| {
                let Some(hash) = block_hash(col, key) else {
                    return Ok(true);
                };
                if let Some(keep) = retained.get(hash) {
                    return Ok(*keep);
                }
                let keep = retain_payload(snapshot, freezer, hash)?;
                retained.put(*hash, keep);
                Ok(keep)
            },
            is_cancelled,
            after_publish,
        )
    }
}
