//! Read every member of an old CF, including side branches and late arrivals.
//! A chain-height cursor alone cannot authorize removal of a physical segment.
use crate::{
    ChainDB, ChainStore,
    archive::{PayloadRoute, invalid},
};
use ckb_db::{IteratorMode, RocksDBSnapshot};
use ckb_db_schema::{COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER};
use ckb_error::Error;
use ckb_freezer::{FreezerController, FreezerReader};
use ckb_types::{core::BlockView, packed, prelude::*};

/// Physical work performed by one maintenance pass. Scanned/archived bytes are
/// not hot-to-hot copies. Layout transitions have no payload-copy operation.
#[derive(Debug, Default)]
pub struct SegmentCollection {
    pub examined_blocks: u64,
    pub archived_blocks: u64,
    pub archived_bytes: u64,
    pub dropped_segments: u64,
    pub blocked_segments: Vec<(u64, &'static str)>,
    pub examined_bytes: u64,
}

enum Member {
    Published,
    Pending(packed::Block),
    Blocked(&'static str),
}

fn member(
    view: &RocksDBSnapshot,
    reader: &FreezerReader,
    start: u64,
    block: BlockView,
    before_epoch: u64,
) -> Result<Member, Error> {
    let hash = block.hash();
    let key = hash.as_slice();
    let Some(raw) = view.get_pinned(COLUMN_BLOCK_ARCHIVE, key)? else {
        return Ok(Member::Blocked("missing route"));
    };
    let archive = match PayloadRoute::decode(raw)? {
        PayloadRoute::Fixed => return Ok(Member::Blocked("fixed override")),
        PayloadRoute::Segment(owner) if owner == start => None,
        PayloadRoute::Segment(_) => {
            return Err(invalid("epoch member has a different segment route"));
        }
        PayloadRoute::Archive(index) => Some(index),
    };
    let header = block.header();
    if header.number() == 0 || header.epoch().number() >= before_epoch {
        return Ok(Member::Blocked("retained epoch or genesis"));
    }
    let Some(ext) = view.get_pinned(COLUMN_BLOCK_EXT, key)? else {
        return Ok(Member::Blocked("missing verification state"));
    };
    let ext = packed::BlockExtReader::from_compatible_slice(&ext).map_err(invalid)?;
    if ext.verified().to_opt().map(bool::from) != Some(true) {
        return Ok(Member::Blocked("unverified"));
    }
    let Some(raw_header) = view.get_pinned(COLUMN_BLOCK_HEADER, key)? else {
        return Ok(Member::Blocked("missing header"));
    };
    let expected: packed::HeaderView = header.into();
    if raw_header.as_ref() != expected.as_slice()
        || block.data().calc_tx_hashes().as_slice() != block.tx_hashes()
        || block.data().calc_tx_witness_hashes().as_slice() != block.tx_witness_hashes()
    {
        return Err(invalid(
            "sealed block identity differs from its stored view",
        ));
    }
    if let Some(index) = archive {
        let bytes = reader
            .retrieve_limited(
                index.record(),
                ckb_freezer::FreezerServiceConfig::default().io_bytes as usize,
            )
            .map_err(invalid)?
            .ok_or_else(|| invalid("indexed epoch archive missing"))?;
        if bytes.as_slice() != block.data().as_slice() {
            return Err(invalid("archived epoch payload differs from sealed bytes"));
        }
        index.check_witnesses(block.tx_witness_hashes())?;
        return Ok(Member::Published);
    }
    Ok(Member::Pending(block.data()))
}

impl ChainDB {
    /// Archive bounded batches, then drop only completely covered sealed segments.
    /// The caller supplies the consensus retention boundary (exclusive epoch).
    pub async fn collect_epoch_segments(
        &self,
        writer: &FreezerController,
        cutoff: impl Fn() -> u64,
        stop: impl Fn() -> bool,
        after_publish: impl Fn(),
    ) -> Result<SegmentCollection, Error> {
        let reader = self
            .archive_reader()
            .ok_or_else(|| invalid("epoch collection needs an archive"))?;
        let windows = self.db().epoch_segments();
        let mut stats = SegmentCollection::default();
        let before_epoch = cutoff();
        for &start in windows.eligible(before_epoch) {
            if stop() {
                return Err(ckb_db::CollectionAbort::Cancelled.into());
            }
            // One immutable snapshot gives a stable membership enumeration. A
            // pending block may be logically edited during append; publication
            // rechecks its live route and bytes. Retirement takes a fresh view.
            let view = self.db().get_snapshot();
            let mut iter = view.segment_iter(start, IteratorMode::Start)?;
            let mut batch = Vec::new();
            let mut bytes = 0;
            let mut blocked = None;
            while let Some(block) = crate::segment_block::next_block(&mut iter)? {
                if stop() {
                    return Err(ckb_db::CollectionAbort::Cancelled.into());
                }
                stats.examined_blocks += 1;
                stats.examined_bytes += block.data().as_slice().len() as u64;
                match member(&view, reader, start, block, before_epoch)? {
                    Member::Published => (),
                    Member::Blocked(reason) => {
                        blocked.get_or_insert(reason);
                    }
                    Member::Pending(block) => {
                        let len = block.as_slice().len();
                        if !batch.is_empty() && (batch.len() >= 512 || bytes + len > 16 << 20) {
                            self.publish_segment_batch(
                                writer,
                                std::mem::take(&mut batch),
                                bytes,
                                &mut stats,
                                &stop,
                            )
                            .await?;
                            bytes = 0;
                        }
                        bytes += len;
                        batch.push(block);
                    }
                }
            }
            iter.status().map_err(invalid)?;
            drop(iter);
            drop(view);
            self.publish_segment_batch(writer, batch, bytes, &mut stats, &stop)
                .await?;
            if stop() {
                return Err(ckb_db::CollectionAbort::Cancelled.into());
            }
            if let Some(reason) = blocked {
                stats.blocked_segments.push((start, reason));
                continue;
            }
            if self.db().retire_epoch_segment(
                start,
                |view| {
                    let mut iter = view.segment_iter(start, IteratorMode::Start)?;
                    while let Some(block) = crate::segment_block::next_block(&mut iter)? {
                        if stop()
                            || !matches!(
                                member(view, reader, start, block, before_epoch)?,
                                Member::Published
                            )
                        {
                            return Ok(false);
                        }
                    }
                    iter.status().map_err(invalid)?;
                    Ok(true)
                },
                || !stop() && windows.eligible(cutoff()).binary_search(&start).is_ok(),
                &after_publish,
            )? {
                stats.dropped_segments += 1;
            } else if stop() {
                return Err(ckb_db::CollectionAbort::Cancelled.into());
            } else {
                stats
                    .blocked_segments
                    .push((start, "coverage or retention changed"));
            }
        }
        Ok(stats)
    }

    async fn publish_segment_batch(
        &self,
        writer: &FreezerController,
        batch: Vec<packed::Block>,
        bytes: usize,
        stats: &mut SegmentCollection,
        stop: &impl Fn() -> bool,
    ) -> Result<(), Error> {
        if batch.is_empty() || stop() {
            return Ok(());
        }
        let count = batch.len();
        writer.append(batch).await?;
        self.recover_archive_with_cancel(stop)?;
        stats.archived_blocks += count as u64;
        stats.archived_bytes += bytes as u64;
        Ok(())
    }
}
