//! Per-tip memoization of chain-cell liveness for block-template building,
//! see [`CellLivenessMemo`]. Split out of `block_assembler/mod.rs`.

use ckb_snapshot::Snapshot;
use ckb_types::{
    core::cell::{CellChecker, TransactionsChecker},
    packed::{Byte32, OutPoint},
};
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

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
#[derive(Default)]
pub(crate) struct CellLivenessMemo {
    pub(crate) tip_hash: Option<Byte32>,
    pub(crate) inner: HashMap<OutPoint, Option<bool>>,
}

impl CellLivenessMemo {
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
        self.inner.insert(out_point.clone(), live);
        live
    }
}

/// Cell checker that memoizes the chain-snapshot fallback per tip while
/// evaluating the in-block overlay fresh on every call.
pub(crate) struct MemoizedChecker<'a> {
    pub(crate) transactions_checker: &'a TransactionsChecker,
    pub(crate) snapshot: &'a Snapshot,
    pub(crate) memo: &'a StdMutex<CellLivenessMemo>,
}

impl CellChecker for MemoizedChecker<'_> {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        // Overlay first, matching `OverlayCellChecker` semantics: in-block
        // producers/consumers change on every update and are never memoized.
        if let Some(live) = self.transactions_checker.is_live(out_point) {
            return Some(live);
        }
        let mut memo = self.memo.lock().expect("cell liveness memo poisoned");
        memo.get_or_load(self.snapshot, out_point)
    }
}
