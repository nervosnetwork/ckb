//! Ownership and writer coordination for replaceable payload column families.
use crate::{Result, internal_error};
use ckb_db_schema::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE,
    COLUMN_NUMBER_HASH, Col,
};
use rocksdb::{
    DBPinnableSlice, OptimisticTransactionDB, Options, OwnedColumnFamily, ReadOptions,
    ops::{Delete, Flush, GetPinned, Put},
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

pub(crate) const PAYLOAD_COLUMNS: [Col; 5] = [
    COLUMN_BLOCK_BODY,
    COLUMN_BLOCK_UNCLE,
    COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_NUMBER_HASH,
    COLUMN_BLOCK_EXTENSION,
];
pub(crate) const GENERATION_KEY: &[u8] = b"freezer/cf-generation";
const PREFIX: &str = "freezer.";

/// Advance RocksDB's persisted WAL boundary after dropping column families.
/// The caller must serialize this with generation publication.
pub(crate) fn checkpoint_retirement(db: &OptimisticTransactionDB) -> Result<()> {
    // DropCF does not advance the minimum WAL number in RocksDB 11.8.1.
    // An empty flush does nothing, so refresh the existing marker first. Only
    // the small default CF is flushed; retired payloads need no new SSTs.
    // Before the first publication, preserve the marker's absence with a
    // tombstone. It still gives the flush a memtable to persist.
    match db.get_pinned(GENERATION_KEY).map_err(internal_error)? {
        Some(marker) => db.put(GENERATION_KEY, marker.as_ref()),
        None => db.delete(GENERATION_KEY),
    }
    .map_err(internal_error)?;
    fail::fail_point!("freezer-gc-retirement-checkpoint", |_| Err(internal_error(
        "injected retirement checkpoint failure"
    )));
    db.flush().map_err(internal_error)
}

pub(crate) fn physical_name(generation: u64, col: Col) -> String {
    if generation == 0 {
        col.to_owned()
    } else {
        format!("{PREFIX}{generation}.{col}")
    }
}

pub(crate) fn logical_name(name: &str) -> Option<Col> {
    let logical = if let Some(suffix) = name.strip_prefix(PREFIX) {
        let (generation, col) = suffix.split_once('.')?;
        if generation.parse::<u64>().ok()? == 0 {
            return None;
        }
        col
    } else {
        name
    };
    PAYLOAD_COLUMNS.into_iter().find(|col| *col == logical)
}

pub(crate) struct Generation {
    pub id: u64,
    pub columns: BTreeMap<String, Arc<OwnedColumnFamily>>,
}

impl Generation {
    pub fn cf(&self, col: Col) -> Result<&Arc<OwnedColumnFamily>> {
        self.columns
            .get(col)
            .ok_or_else(|| internal_error(format!("column {col} not found")))
    }

    pub fn get_pinned(
        &self,
        col: Col,
        key: &[u8],
        options: &ReadOptions,
    ) -> Result<Option<DBPinnableSlice<'static>>> {
        self.cf(col)?
            .get_pinned(key, options)
            .map_err(internal_error)
    }
}

pub(crate) struct Routing {
    pub current: RwLock<Arc<Generation>>,
    pub options: BTreeMap<Col, Options>,
    /// The normal opener gives all non-body payload columns identical options.
    /// Custom option files and the bulk-load opener keep separate templates.
    pub batch_default_payload_cfs: bool,
    pub gate: Arc<WriterGate>,
    /// At most one collector may prepare or publish a replacement.
    pub collector: Mutex<()>,
    /// Weak CF ownership also detects values pinned without an entire snapshot.
    pub retired: Mutex<Vec<(Instant, Vec<Weak<OwnedColumnFamily>>)>>,
}

#[derive(Default)]
struct GateState {
    paused: bool,
    writers: usize,
    failed: bool,
    dirty: Option<DirtyKeys>,
}

pub(crate) struct DirtyKeys {
    pub keys: BTreeSet<(Col, Vec<u8>)>,
    pub bytes: usize,
    limit: usize,
    pub overflow: bool,
}

impl DirtyKeys {
    fn record(&mut self, col: Col, key: &[u8]) {
        if self.overflow {
            return;
        }
        // Count each distinct key's allocation and tree overhead once.
        let entry = (col, key.to_vec());
        if self.keys.contains(&entry) {
            return;
        }
        let cost = key.len().saturating_add(96);
        if cost > self.limit.saturating_sub(self.bytes) {
            self.overflow = true;
        } else {
            self.keys.insert(entry);
            self.bytes += cost;
        }
    }
}

#[derive(Default)]
pub(crate) struct WriterGate {
    state: Mutex<GateState>,
    changed: Condvar,
}

pub(crate) struct WriterPermit {
    gate: Arc<WriterGate>,
}

impl WriterGate {
    pub fn status(&self) -> (usize, bool, usize) {
        let state = self.state.lock().expect("writer gate poisoned");
        (
            state.writers,
            state.paused,
            state.dirty.as_ref().map_or(0, |dirty| dirty.bytes),
        )
    }
    pub fn enter(self: &Arc<Self>) -> WriterPermit {
        let mut state = self.state.lock().expect("writer gate poisoned");
        while state.paused {
            state = self.changed.wait(state).expect("writer gate poisoned");
        }
        state.writers += 1;
        WriterPermit {
            gate: Arc::clone(self),
        }
    }

    // Used only by an exclusive metadata operation after the gate has drained.
    pub fn enter_paused(self: &Arc<Self>) -> WriterPermit {
        let mut state = self.state.lock().expect("writer gate poisoned");
        assert!(state.paused && state.writers == 0);
        state.writers = 1;
        WriterPermit {
            gate: Arc::clone(self),
        }
    }

    pub fn pause(self: &Arc<Self>, timeout: Duration) -> Result<WritePause> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().expect("writer gate poisoned");
        state.paused = true;
        while state.writers != 0 {
            let now = Instant::now();
            if now >= deadline {
                state.paused = false;
                self.changed.notify_all();
                return Err(crate::CollectionAbort::WriterWait.into());
            }
            (state, _) = self
                .changed
                .wait_timeout(state, deadline - now)
                .expect("writer gate poisoned");
        }
        Ok(WritePause {
            gate: Arc::clone(self),
        })
    }

    pub fn check(&self) -> Result<()> {
        if self.state.lock().expect("writer gate poisoned").failed {
            Err(internal_error(
                "uncertain freezer column-family operation; reopen the database before writing",
            ))
        } else {
            Ok(())
        }
    }

    pub fn fail(&self) {
        self.state.lock().expect("writer gate poisoned").failed = true;
    }

    pub fn start_capture(&self, limit: usize) {
        self.state.lock().expect("writer gate poisoned").dirty = Some(DirtyKeys {
            keys: BTreeSet::new(),
            bytes: 0,
            limit,
            overflow: false,
        });
    }

    pub fn finish_capture(&self) -> Option<DirtyKeys> {
        self.state
            .lock()
            .expect("writer gate poisoned")
            .dirty
            .take()
    }

    pub fn capture_overflowed(&self) -> bool {
        self.state
            .lock()
            .expect("writer gate poisoned")
            .dirty
            .as_ref()
            .is_some_and(|dirty| dirty.overflow)
    }
}

impl WriterPermit {
    pub fn check(&self) -> Result<()> {
        self.gate.check()
    }

    pub fn record(&self, col: Col, key: &[u8]) -> Result<()> {
        let mut state = self.gate.state.lock().expect("writer gate poisoned");
        if state.failed {
            return Err(internal_error(
                "uncertain freezer column-family operation; reopen the database before writing",
            ));
        }
        if PAYLOAD_COLUMNS.contains(&col)
            && let Some(dirty) = &mut state.dirty
        {
            dirty.record(col, key);
        }
        Ok(())
    }
}

impl Drop for WriterPermit {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().expect("writer gate poisoned");
        state.writers -= 1;
        if state.writers == 0 {
            self.gate.changed.notify_all();
        }
    }
}

pub(crate) struct WritePause {
    gate: Arc<WriterGate>,
}
impl Drop for WritePause {
    fn drop(&mut self) {
        self.gate.state.lock().expect("writer gate poisoned").paused = false;
        self.gate.changed.notify_all();
    }
}
