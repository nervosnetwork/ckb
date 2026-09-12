//! Fixture adapter compiled inside authority only by the packing benchmark.

use super::{
    model::{Accepted, Entry, Phase, Source},
    packing::{Selection, TemplatePackingLimits},
};
use crate::TxEntry;
use ckb_snapshot::Snapshot;
use std::{collections::BTreeSet, sync::Arc};

/// Identifies the executed production implementation in benchmark receipts.
pub const ADAPTER: &str = "authority_selection_v1";

/// Immutable accepted fixture. Construction is outside selection timing.
pub struct PackingSource {
    owners: Vec<Arc<Entry>>,
    snapshot: Arc<Snapshot>,
    max_ancestors: usize,
}

impl PackingSource {
    /// Project fixture entries into the same immutable owners used by the driver.
    /// The supplied snapshot must classify every fixture proposal as proposed.
    pub fn new(
        entries: &[TxEntry],
        snapshot: Arc<Snapshot>,
        max_ancestors: usize,
    ) -> Result<Self, String> {
        let hashes: BTreeSet<_> = entries
            .iter()
            .map(|entry| entry.transaction().hash())
            .collect();
        if hashes.len() != entries.len() {
            return Err("duplicate fixture transaction".into());
        }
        let mut owners = Vec::with_capacity(entries.len());
        for entry in entries {
            if !snapshot
                .proposals()
                .contains_proposed(&entry.proposal_short_id())
            {
                return Err("fixture proposal is not proposed".into());
            }
            let parents = entry
                .transaction()
                .input_pts_iter()
                .map(|point| point.tx_hash())
                .chain(entry.related_dep_out_points().map(|point| point.tx_hash()))
                .filter(|hash| hashes.contains(hash))
                .collect();
            owners.push(Arc::new(Entry {
                transaction: Arc::new(entry.transaction().clone()),
                arrival: entry.timestamp,
                source: Source::Local,
                phase: Phase::Accepted(Accepted {
                    transaction: Arc::clone(&entry.rtx),
                    cycles: entry.cycles,
                    fee: entry.fee,
                    size: entry.size,
                    timestamp: entry.timestamp,
                    parents,
                    context_sensitive: false,
                    #[cfg(any(test, feature = "internal"))]
                    forced_status: None,
                }),
            }));
        }
        Ok(Self {
            owners,
            snapshot,
            max_ancestors,
        })
    }

    /// Execute one template's production selection, including derived graph
    /// construction and destruction. No authoritative store lock is measured.
    pub fn select(&self, bytes: usize, cycles: u64) -> Result<Vec<TxEntry>, String> {
        Selection::new(&self.owners, &self.snapshot, self.max_ancestors)
            .map_err(|error| error.to_string())?
            .pack_transactions(TemplatePackingLimits::new(bytes, cycles))
            .map_err(|error| format!("packing failed: {error:?}"))
    }
}
