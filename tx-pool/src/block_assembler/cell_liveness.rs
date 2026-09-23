//! Per-tip memoization of chain-cell liveness for block-template building,
//! see [`CellLivenessMemo`]. Split out of `block_assembler/mod.rs`.

use ckb_snapshot::Snapshot;
use ckb_types::{
    core::cell::{CellChecker, TransactionsChecker},
    packed::{Byte32, OutPoint},
    prelude::Entity,
};
use ckb_util::Mutex;
use lru::LruCache;
use std::collections::hash_map::RandomState;

use crate::util::compact_packed;

/// Per-tip memo of chain-cell liveness results, shared across block-template
/// updates.
///
/// `calc_dao` checks every candidate transaction's inputs against the chain
/// snapshot on every template update, and those lookups can hit RocksDB. The
/// snapshot is immutable within a tip, so the results are memoized here and
/// reused until the tip changes (detected automatically via the tip-hash
/// stamp, no explicit invalidation is required).
///
/// Only the chain-snapshot fallback is memoized: the in-block overlay
/// (`TransactionsChecker`) changes on every update and is always evaluated
/// fresh.
pub(crate) struct CellLivenessMemo {
    pub(crate) tip_hash: Option<Byte32>,
    /// The entry cap follows the block byte limit. Refreshing hits and evicting
    /// one least-recently-used key preserves hot lookups through bounded churn.
    /// Keys have compact backing; values include cached unknown results.
    pub(crate) inner: LruCache<OutPoint, Option<bool>, RandomState>,
}

impl CellLivenessMemo {
    pub(crate) fn for_block_bytes(max_block_bytes: usize) -> Self {
        let packed_out_point_bytes = OutPoint::default().as_slice().len().max(1);
        // Set the bound without eagerly reserving a maximum-sized hash table.
        let mut inner = LruCache::with_hasher(0, RandomState::new());
        inner.resize(max_block_bytes.div_ceil(packed_out_point_bytes).max(1));
        Self {
            tip_hash: None,
            inner,
        }
    }

    pub(crate) fn get_or_load(
        &mut self,
        snapshot: &Snapshot,
        out_point: &OutPoint,
    ) -> Option<bool> {
        let tip_hash = snapshot.tip_hash();
        if self.tip_hash.as_ref() != Some(&tip_hash) {
            self.tip_hash = Some(tip_hash);
            self.inner.clear();
        }
        if let Some(&live) = self.inner.get(out_point) {
            return live;
        }
        let live = snapshot.is_live(out_point);
        self.inner.put(compact_packed(out_point), live);
        live
    }
}

impl Default for CellLivenessMemo {
    fn default() -> Self {
        // Tests and standalone helpers get a useful bounded memo. Production
        // constructs it from the active consensus block-byte limit.
        Self::for_block_bytes(1 << 20)
    }
}

/// Cell checker that memoizes the chain-snapshot fallback per tip while
/// evaluating the in-block overlay fresh on every call.
pub(crate) struct MemoizedChecker<'a> {
    pub(crate) transactions_checker: &'a TransactionsChecker,
    pub(crate) snapshot: &'a Snapshot,
    pub(crate) memo: &'a Mutex<CellLivenessMemo>,
}

impl CellChecker for MemoizedChecker<'_> {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        // Overlay first, matching `OverlayCellChecker` semantics: in-block
        // producers/consumers change on every update and are never memoized.
        if let Some(live) = self.transactions_checker.is_live(out_point) {
            return Some(live);
        }
        let mut memo = self.memo.lock();
        memo.get_or_load(self.snapshot, out_point)
    }
}
