use arc_swap::{ArcSwap, Guard};
pub use ckb_snapshot::Snapshot;
use std::sync::Arc;

pub struct SnapshotMgr {
    inner: ArcSwap<Snapshot>,
}

impl SnapshotMgr {
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        SnapshotMgr {
            inner: ArcSwap::new(snapshot),
        }
    }

    pub fn load(&self) -> Guard<Arc<Snapshot>> {
        self.inner.load()
    }

    pub fn store(&self, snapshot: Arc<Snapshot>) {
        self.inner.store(snapshot);
    }
}
