//! Copy retained payloads, reconcile concurrent changes, publish, then drop old CFs.
use crate::generation::{
    GENERATION_KEY, Generation, PAYLOAD_COLUMNS, checkpoint_retirement, physical_name,
};
use crate::{DBIterator, Result, RocksDB, RocksDBSnapshot, internal_error};
use ckb_db_schema::{
    COLUMN_BLOCK_BODY, COLUMN_META, Col, META_ARCHIVE_COLLECTED, META_ARCHIVE_NEXT_RECORD,
};
use rocksdb::{
    IteratorMode, ReadOptions, WriteBatch, WriteOptions,
    ops::{FlushWal, WriteOps},
};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Expected reasons to defer collection while preserving the current generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CollectionAbort {
    /// Shutdown or caller cancellation.
    Cancelled,
    /// Readers still own too many retired generations.
    RetainedReaders,
    /// Existing writers did not finish in time.
    WriterWait,
    /// Concurrent mutations exceeded the dirty-key accounting budget.
    DirtyKeys,
    /// Reconciliation did not finish in time.
    Reconciliation,
}

impl CollectionAbort {
    /// Extract a deferral reason from the database's existing error envelope.
    pub fn from_error(error: &ckb_error::Error) -> Option<Self> {
        error
            .downcast_ref::<ckb_error::InternalError>()?
            .downcast_ref::<Self>()
            .copied()
    }
}

impl std::fmt::Display for CollectionAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "freezer collection cancelled",
            Self::RetainedReaders => {
                "freezer collection deferred while older readers retain column families"
            }
            Self::WriterWait => "freezer collection timed out waiting for writers",
            Self::DirtyKeys => "freezer collection dirty-key limit exceeded",
            Self::Reconciliation => "freezer collection reconciliation time limit exceeded",
        })
    }
}

impl std::error::Error for CollectionAbort {}

impl From<CollectionAbort> for ckb_error::Error {
    fn from(reason: CollectionAbort) -> Self {
        ckb_error::InternalErrorKind::DataCorrupted
            .because(reason)
            .into()
    }
}

/// Resource limits for one payload collection attempt.
#[derive(Clone, Debug)]
pub struct CollectionOptions {
    /// Maximum in-memory accounting for keys changed while copying.
    pub max_dirty_bytes: usize,
    /// Defer another copy while this many old generations remain pinned.
    pub max_retired_generations: usize,
    /// Flush target for serialized copy/reconciliation batches. A batch can
    /// exceed this target by one source entry, including a large transaction.
    pub batch_bytes: usize,
    /// Abort if existing writers cannot drain within this interval.
    pub writer_wait: Duration,
    /// Abort reconciliation before publication if it takes longer than this.
    /// The final WAL sync and publication callback are not interruptible.
    pub reconcile_timeout: Duration,
}

impl Default for CollectionOptions {
    fn default() -> Self {
        Self {
            max_dirty_bytes: 32 << 20,
            max_retired_generations: 2,
            batch_bytes: 1 << 20,
            writer_wait: Duration::from_millis(100),
            reconcile_timeout: Duration::from_millis(100),
        }
    }
}

/// Work completed by a published collection.
#[derive(Debug, Default)]
pub struct CollectionStats {
    /// Newly published generation identifier.
    pub generation: u64,
    /// Archive index frontier captured in the same snapshot as the source data.
    pub archive_frontier: u64,
    /// Entries retained from the copy snapshot.
    pub copied: u64,
    /// Key/value bytes retained from the source snapshot, excluding WAL framing.
    pub copied_bytes: u64,
    /// Entries omitted because the caller proved that they were archived.
    pub archived: u64,
    /// Concurrently changed keys reconciled before publication.
    pub reconciled: usize,
    /// Peak accounting for distinct changed keys, including estimated tree costs.
    pub dirty_bytes: usize,
    /// Largest serialized batch, including a single entry that exceeds the target.
    pub max_batch_bytes: usize,
    /// Combined time waiting for writers at both collection barriers.
    pub writer_wait_time: Duration,
    /// Time to create the replacement column families.
    pub creation_time: Duration,
    /// Source scanning, archive validation and target copying time.
    pub copy_time: Duration,
    /// Time syncing copied WAL data while foreground writers may still run.
    pub preparation_sync_time: Duration,
    /// Time reconciling concurrent changes with writers paused.
    pub reconciliation_time: Duration,
    /// Time syncing the final metadata and reconciliation batch.
    pub publication_sync_time: Duration,
    /// Time spent reconciling, syncing and publishing with writers paused.
    pub publication_time: Duration,
    /// Time to drop old CFs and persist their WAL boundary, excluding reader release.
    pub retirement_time: Duration,
    /// Old CFs removed from the MANIFEST; old read views may still pin their files.
    pub retired_columns: usize,
}

/// Current writer occupancy and old-generation retention, sampled for diagnostics.
#[derive(Debug)]
pub struct CollectionStatus {
    /// Live writer permits, including batches and transactions.
    pub active_writers: usize,
    /// Whether admission of new writers is paused.
    pub writers_paused: bool,
    /// Estimated memory for distinct keys changed during the current copy.
    pub dirty_bytes: usize,
    /// Retired generations still held by read views or pinned values.
    pub pinned_retired_generations: usize,
    /// Time since the oldest still-pinned generation was retired.
    pub oldest_retired_age: Duration,
}

impl RocksDB {
    /// Replace all payload CFs while ordinary writes continue in their current CFs.
    ///
    /// Every `retain` call receives the same initial snapshot and must use its durable
    /// archive membership view. Returning false
    /// asserts that the complete immutable block is recoverable from that archive.
    /// Concurrently changed keys are always reconciled from the latest source:
    /// a logical cold-block edit may have restored its payload to hot storage.
    /// Any copy error or exceeded resource limit leaves the old generation active.
    /// Uncertain native Create/Drop/publication errors prevent further writes
    /// until reopening; an error during retirement may follow a successful switch.
    ///
    /// `after_publish` refreshes application read views before writers resume. It
    /// may create snapshots but must not begin a transaction or write batch.
    pub fn collect_payload(
        &self,
        options: &CollectionOptions,
        retain: impl FnMut(&RocksDBSnapshot, Col, &[u8], &[u8]) -> Result<bool>,
        after_publish: impl FnOnce(),
    ) -> Result<CollectionStats> {
        self.collect_payload_with_cancel(options, retain, || false, after_publish)
    }

    /// Collect with cooperative cancellation before publication begins. Once the
    /// publication WAL write starts, routing and retirement must finish normally.
    pub fn collect_payload_with_cancel(
        &self,
        options: &CollectionOptions,
        mut retain: impl FnMut(&RocksDBSnapshot, Col, &[u8], &[u8]) -> Result<bool>,
        is_cancelled: impl Fn() -> bool,
        after_publish: impl FnOnce(),
    ) -> Result<CollectionStats> {
        let check_cancelled = || -> Result<()> {
            if is_cancelled() {
                Err(CollectionAbort::Cancelled.into())
            } else {
                Ok(())
            }
        };
        if options.batch_bytes == 0
            || options.max_dirty_bytes == 0
            || options.max_retired_generations == 0
        {
            return Err(internal_error("collection byte limits must be positive"));
        }
        let _collector = self
            .routing
            .collector
            .lock()
            .expect("collector lock poisoned");
        self.routing.gate.check()?;
        check_cancelled()?;
        if self.pinned_retired_generations() >= options.max_retired_generations {
            return Err(CollectionAbort::RetainedReaders.into());
        }
        let source = self.generation();
        let creating = Instant::now();
        let target = self.create_generation(&source)?;
        let creation_time = creating.elapsed();
        let mut attempted_publish = false;
        let result = (|| {
            check_cancelled()?;
            let waiting = Instant::now();
            let pause = self.routing.gate.pause(options.writer_wait)?;
            let initial_wait = waiting.elapsed();
            // Every preexisting writer has finished. Capturing before releasing the
            // pause covers writers created during both copying and reconciliation.
            self.routing.gate.start_capture(options.max_dirty_bytes);
            let snapshot = self.get_snapshot();
            drop(pause);

            let archive_frontier = snapshot
                .get_pinned(COLUMN_META, META_ARCHIVE_NEXT_RECORD)?
                .map(|raw| {
                    <[u8; 8]>::try_from(raw.as_ref())
                        .map(u64::from_le_bytes)
                        .map_err(internal_error)
                })
                .transpose()?
                .unwrap_or(1);
            let mut stats = CollectionStats {
                generation: target.id,
                archive_frontier,
                writer_wait_time: initial_wait,
                creation_time,
                ..Default::default()
            };
            let copying = Instant::now();
            let mut batch = WriteBatch::default();
            let mut read_options = ReadOptions::default();
            read_options.set_total_order_seek(true);
            for col in PAYLOAD_COLUMNS {
                let mut iter = snapshot.iter_opt(col, IteratorMode::Start, &read_options)?;
                for (key, value) in iter.by_ref() {
                    check_cancelled()?;
                    if self.routing.gate.capture_overflowed() {
                        return Err(CollectionAbort::DirtyKeys.into());
                    }
                    if retain(&snapshot, col, &key, &value)? {
                        batch
                            .put_cf(target.cf(col)?, &key, &value)
                            .map_err(internal_error)?;
                        stats.copied += 1;
                        stats.copied_bytes += (key.len() + value.len()) as u64;
                    } else {
                        stats.archived += 1;
                    }
                    if batch.size_in_bytes() >= options.batch_bytes {
                        self.flush_copy(&mut batch, &mut stats)?;
                    }
                }
                // A truncated or corrupt source must never become a published copy.
                iter.status().map_err(internal_error)?;
            }
            self.flush_copy(&mut batch, &mut stats)?;
            fail::fail_point!("freezer-gc-after-copy", |_| Err(internal_error(
                "injected failure after copy"
            )));
            drop(snapshot);
            stats.copy_time = copying.elapsed();

            // Move the bulk of WAL persistence outside the final writer pause.
            // Concurrent writes and reconciliation still require the sync below.
            check_cancelled()?;
            let syncing = Instant::now();
            self.inner.flush_wal(true).map_err(internal_error)?;
            stats.preparation_sync_time = syncing.elapsed();
            fail::fail_point!("freezer-gc-after-prepare-sync", |_| Err(internal_error(
                "injected failure after preparatory WAL sync"
            )));
            check_cancelled()?;
            let waiting = Instant::now();
            let _pause = self.routing.gate.pause(options.writer_wait)?;
            stats.writer_wait_time += waiting.elapsed();
            let started = Instant::now();
            let dirty = self
                .routing
                .gate
                .finish_capture()
                .expect("capture started above");
            if dirty.overflow {
                return Err(CollectionAbort::DirtyKeys.into());
            }
            stats.reconciled = dirty.keys.len();
            stats.dirty_bytes = dirty.bytes;
            for (col, key) in dirty.keys {
                check_cancelled()?;
                if started.elapsed() >= options.reconcile_timeout {
                    return Err(CollectionAbort::Reconciliation.into());
                }
                match source.get_pinned(col, &key, &read_options)? {
                    Some(value) => {
                        batch
                            .put_cf(target.cf(col)?, &key, &value)
                            .map_err(internal_error)?;
                    }
                    _ => {
                        // Only foreground mutations create these few tombstones in
                        // the replacement; archived history is omitted during copy.
                        batch
                            .delete_cf(target.cf(col)?, &key)
                            .map_err(internal_error)?;
                    }
                }
                if batch.size_in_bytes() >= options.batch_bytes {
                    self.flush_copy(&mut batch, &mut stats)?;
                }
            }
            if started.elapsed() >= options.reconcile_timeout {
                return Err(CollectionAbort::Reconciliation.into());
            }
            check_cancelled()?;
            fail::fail_point!("freezer-gc-after-reconcile", |_| Err(internal_error(
                "injected failure after reconciliation"
            )));
            stats.reconciliation_time = started.elapsed();
            batch
                .put_cf(
                    target.cf(COLUMN_META)?,
                    META_ARCHIVE_COLLECTED,
                    stats.archive_frontier.to_le_bytes(),
                )
                .map_err(internal_error)?;
            batch
                .put(GENERATION_KEY, target.id.to_le_bytes())
                .map_err(internal_error)?;
            let mut write_options = WriteOptions::default();
            write_options.set_sync(true);
            fail::fail_point!("freezer-gc-before-publish", |_| Err(internal_error(
                "injected failure before publication"
            )));
            attempted_publish = true;
            stats.max_batch_bytes = stats.max_batch_bytes.max(batch.size_in_bytes());
            // Syncing the publication batch also syncs all earlier copy batches in
            // the shared WAL. No generation becomes authoritative before this.
            let syncing = Instant::now();
            if let Err(error) = self.inner.write_opt(&batch, &write_options) {
                self.routing.gate.fail();
                return Err(internal_error(error));
            }
            stats.publication_sync_time = syncing.elapsed();
            fail::fail_point!("freezer-gc-after-publish-sync", |_| {
                self.routing.gate.fail();
                Err(internal_error(
                    "injected uncertain publication error; reopen",
                ))
            });
            // Source CFs remain unchanged while writers are paused, so readers
            // may capture the old generation during the WAL sync. Only the swap
            // must exclude snapshot creation: a new route needs a new sequence.
            let mut route = self
                .routing
                .current
                .write()
                .expect("generation lock poisoned");
            *route = Arc::clone(&target);
            drop(route);
            after_publish();
            fail::fail_point!("freezer-gc-after-route", |_| {
                self.routing.gate.fail();
                Err(internal_error(
                    "injected error after routing publication; reopen",
                ))
            });
            stats.publication_time = started.elapsed();
            Ok(stats)
        })();
        self.routing.gate.finish_capture();
        match result {
            Ok(mut stats) => {
                let retiring = Instant::now();
                self.retire(&source)?;
                self.routing
                    .retired
                    .lock()
                    .expect("retired generations poisoned")
                    .push((
                        Instant::now(),
                        PAYLOAD_COLUMNS
                            .iter()
                            .map(|col| Arc::downgrade(&source.columns[*col]))
                            .collect(),
                    ));
                stats.retired_columns = PAYLOAD_COLUMNS.len();
                stats.retirement_time = retiring.elapsed();
                Ok(stats)
            }
            Err(error) => {
                if !attempted_publish && let Err(cleanup) = self.retire(&target) {
                    return Err(internal_error(format!(
                        "{error}; temporary generation cleanup failed: {cleanup}"
                    )));
                }
                // After an uncertain sync both generations must remain in the
                // MANIFEST. Reopening reads the marker and resolves the outcome.
                Err(error)
            }
        }
    }

    fn create_generation(&self, source: &Generation) -> Result<Arc<Generation>> {
        if self.routing.options.len() != PAYLOAD_COLUMNS.len() {
            return Err(internal_error(
                "payload collection requires all chain payload columns",
            ));
        }
        let id = source
            .id
            .checked_add(1)
            .ok_or_else(|| internal_error("freezer generation overflow"))?;
        let mut columns = source.columns.clone();
        let groups = if self.routing.batch_default_payload_cfs {
            let (body, other): (Vec<_>, Vec<_>) = PAYLOAD_COLUMNS
                .into_iter()
                .partition(|col| *col == COLUMN_BLOCK_BODY);
            vec![body, other]
        } else {
            PAYLOAD_COLUMNS.into_iter().map(|col| vec![col]).collect()
        };
        for group in groups {
            let names: Vec<_> = group.iter().map(|col| physical_name(id, col)).collect();
            let handles = self
                .inner
                .create_owned_cfs(&names, &self.routing.options[group[0]])
                .map_err(|error| {
                    // Native CreateCF can return an error after creating the CF.
                    // Preserve the complete namespace until reopening resolves it.
                    self.routing.gate.fail();
                    internal_error(error)
                })?;
            for (col, column) in group.into_iter().zip(handles) {
                columns.insert(col.to_owned(), column);
            }
            fail::fail_point!("freezer-gc-after-create", |_| {
                self.routing.gate.fail();
                Err(internal_error("injected failure after CF creation; reopen"))
            });
        }
        Ok(Arc::new(Generation { id, columns }))
    }

    fn flush_copy(&self, batch: &mut WriteBatch, stats: &mut CollectionStats) -> Result<()> {
        if !batch.is_empty() {
            stats.max_batch_bytes = stats.max_batch_bytes.max(batch.size_in_bytes());
            self.inner.write(batch).map_err(internal_error)?;
            fail::fail_point!("freezer-gc-after-copy-batch", |_| Err(internal_error(
                "injected failure after copy batch"
            )));
            batch.clear().map_err(internal_error)?;
        }
        Ok(())
    }

    fn retire(&self, generation: &Generation) -> Result<()> {
        // Fixed columns are shared between generations and must never be dropped.
        for col in PAYLOAD_COLUMNS {
            let column = &generation.columns[col];
            fail::fail_point!("freezer-gc-before-drop", |_| {
                self.routing.gate.fail();
                Err(internal_error(
                    "injected failure before CF retirement; reopen",
                ))
            });
            if let Err(error) = column.drop_from_database() {
                // Native DropCF may already have succeeded. An immediate retry
                // could only report "already dropped" forever; reopen to resolve.
                self.routing.gate.fail();
                return Err(internal_error(format!(
                    "uncertain drop of {}: {error}; reopen the database",
                    column.name()
                )));
            }
            fail::fail_point!("freezer-gc-after-drop", |_| {
                self.routing.gate.fail();
                Err(internal_error(
                    "injected uncertain CF retirement error; reopen",
                ))
            });
        }
        checkpoint_retirement(&self.inner).inspect_err(|_| {
            // Retirement succeeded, but WAL reclamation must not remain stalled
            // while foreground writes continue. Reopen before accepting writes.
            self.routing.gate.fail();
        })
    }
}
