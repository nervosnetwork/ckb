//! Publish and retire whole epoch columns. Neither layout transition has a payload write.
use crate::{
    CollectionAbort, Result, RocksDB, RocksDBSnapshot, RocksDBTransaction,
    generation::checkpoint_retirement, internal_error,
};
use ckb_db_schema::{
    COLUMN_BLOCK_UNCLE,
    freezer::{SEGMENTS_KEY, Segments, segment_name},
};
use rocksdb::{
    IteratorMode, ReadOptions, WriteOptions,
    ops::{IterateCF, Put, PutCF},
};
use std::sync::Arc;
use std::time::{Duration, Instant};

impl RocksDB {
    /// Read from the currently published segment. Multi-key store reads use a snapshot.
    pub fn segment_get(
        &self,
        start: u64,
        hash: &[u8],
    ) -> Result<Option<crate::DBPinnableSlice<'static>>> {
        self.generation()
            .named(&segment_name(start))?
            .get_pinned(hash, &ReadOptions::default())
            .map_err(internal_error)
    }
    /// Scan rows from a currently published segment.
    pub fn segment_iter(&self, start: u64, mode: IteratorMode) -> Result<crate::iter::DBIter<'_>> {
        Ok(self
            .generation()
            .named(&segment_name(start))?
            .iterator_opt(mode, &ReadOptions::default()))
    }

    /// Snapshot of the published epoch windows. Empty denotes the legacy layout.
    pub fn epoch_segments(&self) -> Arc<Segments> {
        self.generation().segments.clone()
    }

    /// Live SST file sizes for a set of held windows in one published layout.
    pub fn epoch_sst_bytes(&self, starts: impl Iterator<Item = u64>) -> Result<u64> {
        use rocksdb::ops::GetPropertyCF;
        let view = self.generation();
        let mut bytes = 0u64;
        for start in starts {
            let cf = view.named(&segment_name(start))?;
            let size = self
                .inner
                .property_int_value_cf(cf, "rocksdb.live-sst-files-size")
                .map_err(internal_error)?
                .ok_or_else(|| internal_error("live SST size unavailable"))?;
            bytes = bytes.saturating_add(size);
        }
        Ok(bytes)
    }

    /// Ensure new blocks can enter this window. Repeating or requesting an old
    /// window never creates a replacement for a retired segment.
    pub fn open_epoch_segment(&self, start: u64) -> Result<()> {
        let current = self.epoch_segments();
        if start != current.start(start) {
            return Err(internal_error("invalid epoch window boundary"));
        }
        if current.active().is_some_and(|last| start <= last) {
            return Ok(());
        }
        let _collector = self.routing.collector.lock().expect("collector lock");
        self.check_writable()?;
        let old = self.generation();
        if old.segments.active().is_some_and(|last| start <= last) {
            return Ok(());
        }
        let layout = old
            .segments
            .open(start)
            .ok_or_else(|| internal_error("invalid epoch window boundary"))?;
        let _pause = self.routing.gate.pause(Duration::from_secs(5))?;
        let opts = self
            .routing
            .options
            .get(COLUMN_BLOCK_UNCLE)
            .ok_or_else(|| internal_error("missing epoch column options"))?;
        let name = segment_name(start);
        let cf = self.inner.create_owned_cf(&name, opts).map_err(|e| {
            self.routing.gate.fail();
            internal_error(e)
        })?;
        let mut next = (*old).clone();
        next.columns.insert(name, cf);
        next.segments = Arc::new(layout);
        fail::fail_point!("epoch-after-create", |_| {
            self.routing.gate.fail();
            Err(internal_error("reopen after epoch creation"))
        });
        self.publish_segments(next)
    }

    fn publish_segments(&self, next: crate::generation::Generation) -> Result<()> {
        let marker = next
            .segments
            .encode()
            .ok_or_else(|| internal_error("cannot publish an empty epoch layout"))?;
        let mut opts = WriteOptions::default();
        opts.set_sync(true);
        let mut route = self.routing.current.write().expect("generation lock");
        self.inner
            .put_opt(SEGMENTS_KEY, marker, &opts)
            .map_err(|err| {
                self.routing.gate.fail();
                internal_error(err)
            })?;
        *route = Arc::new(next);
        Ok(())
    }

    /// Verify pinned immutable rows while the node runs. Sealed membership cannot grow, and an archived member may only
    /// move to a materialized fixed override, never back into a sealed route.
    /// This makes the proof monotone; the final pause publishes metadata only.
    pub fn retire_epoch_segment(
        &self,
        start: u64,
        verify: impl FnOnce(&RocksDBSnapshot) -> Result<bool>,
        still_eligible: impl FnOnce() -> bool,
        after_publish: impl FnOnce(),
    ) -> Result<bool> {
        // A sealed segment cannot gain rows or regain a hot route. Pin its
        // membership for the scan without delaying a concurrent window open.
        {
            let view = self.get_snapshot();
            let layout = &view.generation.segments;
            if layout.active() == Some(start)
                || layout.windows().binary_search(&start).is_err()
                || !verify(&view)?
            {
                return Ok(false);
            }
        }
        let _collector = self.routing.collector.lock().expect("collector lock");
        self.check_writable()?;
        let old = self.generation();
        let Some(layout) = old.segments.retire(start) else {
            return Ok(false);
        };
        if self.pinned_retired_generations()
            >= crate::CollectionOptions::default().max_retired_generations
        {
            return Err(CollectionAbort::RetainedReaders.into());
        }
        let _pause = self.routing.gate.pause(Duration::from_millis(100))?;
        if !still_eligible() {
            return Ok(false);
        }
        let mut next = (*old).clone();
        let column = next
            .columns
            .remove(&segment_name(start))
            .ok_or_else(|| internal_error("missing sealed column"))?;
        next.segments = Arc::new(layout);
        self.publish_segments(next)?;
        fail::fail_point!("epoch-after-retire-marker", |_| {
            self.routing.gate.fail();
            Err(internal_error("reopen after epoch retirement publication"))
        });
        after_publish();
        column.drop_from_database().map_err(|e| {
            self.routing.gate.fail();
            internal_error(e)
        })?;
        checkpoint_retirement(&self.inner).inspect_err(|_| self.routing.gate.fail())?;
        self.routing
            .retired
            .lock()
            .expect("retired generations")
            .push((Instant::now(), vec![Arc::downgrade(&column)]));
        Ok(true)
    }
}

impl RocksDBSnapshot {
    /// Iterate a particular live segment under this snapshot's sequence and layout.
    pub fn segment_iter(&self, start: u64, mode: IteratorMode) -> Result<crate::iter::DBIter<'_>> {
        self.iterator_cf(self.generation.named(&segment_name(start))?, mode)
            .map_err(internal_error)
    }
    /// Read a payload with the snapshot's sequence and CF ownership.
    pub fn segment_get(
        &self,
        start: u64,
        key: &[u8],
    ) -> Result<Option<crate::DBPinnableSlice<'_>>> {
        let mut opts = ReadOptions::default();
        unsafe { opts.set_snapshot(self) };
        self.generation
            .named(&segment_name(start))?
            .get_pinned(key, &opts)
            .map_err(internal_error)
    }
}

impl RocksDBTransaction {
    /// Segment receiving new, non-late blocks in this writer's pinned layout.
    pub fn writable_segment(&self, epoch: u64) -> Option<u64> {
        let layout = &self.generation.segments;
        layout
            .active()
            .filter(|&active| layout.start(epoch) <= active)
    }

    /// Only the active segment is writable. Published sealed segments are immutable.
    pub fn put_segment(&self, start: u64, key: &[u8], value: &[u8]) -> Result<()> {
        self.permit.check()?;
        if self.generation.segments.active() != Some(start) {
            return Err(internal_error("epoch segment is sealed"));
        }
        self.inner
            .put_cf(self.generation.named(&segment_name(start))?, key, value)
            .map_err(internal_error)
    }
}
