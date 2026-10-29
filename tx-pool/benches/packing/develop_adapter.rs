//! Default-off benchmark access to develop's production template selector.
use crate::{
    TxEntry,
    component::{
        pool_map::{PoolMap, Status},
        tx_selector::TxSelector,
    },
};
use ckb_snapshot::Snapshot;
use std::sync::Arc;

/// Identifies the executed production implementation in benchmark receipts.
pub const ADAPTER: &str = "develop_tx_selector_v1";

/// The production pre-maintained pool index; setup cost is reported separately.
pub struct PackingSource {
    pool: PoolMap,
}

impl PackingSource {
    /// Insert fixture transactions in causal order, rejecting duplicate/eviction.
    pub fn new(
        entries: &[TxEntry],
        snapshot: Arc<Snapshot>,
        max_ancestors: usize,
    ) -> Result<Self, String> {
        let mut pool = PoolMap::new(max_ancestors);
        for entry in entries {
            if !snapshot
                .proposals()
                .contains_proposed(&entry.proposal_short_id())
            {
                return Err("fixture proposal is not proposed".into());
            }
            let (inserted, evicted) = pool
                .add_entry(entry.clone(), Status::Proposed)
                .map_err(|error| error.to_string())?;
            if !inserted || !evicted.is_empty() {
                return Err("fixture insertion duplicated or evicted an entry".into());
            }
        }
        Ok(Self { pool })
    }

    /// Exactly the selector called by develop's TxPool::package_txs.
    pub fn select(&self, bytes: usize, cycles: u64) -> Result<Vec<TxEntry>, String> {
        Ok(TxSelector::new(&self.pool).txs_to_commit(bytes, cycles).0)
    }
}
