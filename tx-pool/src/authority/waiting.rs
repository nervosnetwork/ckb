//! Current readiness and bounded wake plans for waiting and replaced owners.

use super::{
    model::{DependencyKey, Entry, Error, Phase, Source},
    notice::{Class, Effect},
    store::{Plan, Store},
};
use crate::error::Reject;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use std::sync::Arc;

/// Test current readiness after an availability hint. The final waiter commit
/// carries these producer/spender reads and the captured lifecycle revision.
pub(super) fn available(
    store: &Store,
    snapshot: &Snapshot,
    key: &DependencyKey,
    reads: &mut super::store::ReadSet,
) -> Result<bool, Error> {
    use ckb_types::core::cell::{CellProvider, CellStatus, HeaderChecker};
    match key {
        DependencyKey::Header(hash) => Ok(snapshot.check_valid(hash).is_ok()),
        DependencyKey::Cell(point) => {
            if store.spender(point, reads)?.is_some() {
                return Ok(false);
            }
            if let Some(owner) = store.get(&point.tx_hash(), reads)?
                && owner.accepted().is_some()
            {
                let index: u32 = point.index().into();
                return Ok((index as usize) < owner.transaction.outputs().len());
            }
            Ok(matches!(snapshot.cell(point, false), CellStatus::Live(_)))
        }
    }
}
/// A trusted missing input can wait only for a known, not-yet-accepted
/// producer with that exact output. Absence is terminal on its current cut.
pub(super) fn pending_producer(
    store: &Store,
    point: &ckb_types::packed::OutPoint,
    reads: &mut super::store::ReadSet,
) -> Result<bool, Error> {
    let index: u32 = point.index().into();
    Ok(store.get(&point.tx_hash(), reads)?.is_some_and(|owner| {
        owner.preaccepted() && (index as usize) < owner.transaction.outputs().len()
    }))
}
pub(super) fn wake(
    store: &Store,
    cursor: &mut Option<DependencyKey>,
) -> Result<Option<Plan>, Error> {
    let (view, snapshot) = store.snapshot();
    let Some(page) = store.wake_page(cursor) else {
        return Ok(None);
    };
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.maintenance.wake").entered();
    let mut plan = Plan::new(view, Class::Trusted);
    for hash in &page.hashes {
        let Some(entry) = store.get(hash, &mut plan.reads)? else {
            continue;
        };
        let (keys, require_all, history) = match &entry.phase {
            Phase::Waiting(keys) => (keys, true, false),
            Phase::Replaced {
                triggers,
                require_all,
            } => (triggers, *require_all, true),
            _ => continue,
        };
        let mut any_ready = false;
        let mut all_ready = true;
        let mut lost = None;
        for key in keys {
            let ready =
                crate::util::block_offload(|| available(store, &snapshot, key, &mut plan.reads))?;
            any_ready |= ready;
            all_ready &= ready;
            if !ready
                && !history
                && entry.source.requires_known_producer()
                && let DependencyKey::Cell(point) = key
                && !pending_producer(store, point, &mut plan.reads)?
            {
                lost = Some(point.clone());
                break;
            }
        }
        if let Some(point) = lost {
            plan.effects.push(Effect::rejected(
                &entry.hash(),
                Reject::Resolve(OutPointError::Unknown(point)),
                None,
                entry.source.residency_peer().is_some(),
            )?);
            plan.edit(Some(entry), None)?;
        } else if (require_all && all_ready) || (!require_all && any_ready) {
            let after = if history {
                Arc::new(Entry {
                    transaction: Arc::clone(&entry.transaction),
                    arrival: entry.arrival,
                    source: Source::Recovery,
                    phase: Phase::Resolve,
                })
            } else {
                entry.with_phase(Phase::Resolve)
            };
            plan.edit(Some(entry), Some(after))?;
        }
    }
    plan.advance(page);
    Ok(Some(plan))
}
