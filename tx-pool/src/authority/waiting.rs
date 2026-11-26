//! Readiness checks and bounded paged wake commits for waiting and replaced owners.
//!
//! Chain-snapshot DB lookups run strictly off-lock (`block_offload`). The final
//! wake commit then rechecks live pool state and the lifecycle revision under
//! one short write guard before requeuing ready owners and advancing the cursor.

use super::{
    model::{Entry, Error, OutPointKey, Phase, Source, hash_key},
    notice::{Batch, Class, Effect},
    state::{AuthorityState, OwnerEdit},
    store::{Store, WakePageSnapshot},
};
use crate::error::Reject;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        cell::{CellProvider, CellStatus},
        error::OutPointError,
    },
    packed::OutPoint,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// A trusted missing input can wait only for a known, not-yet-accepted
/// producer with that exact output. Absence is terminal on its current cut.
pub(super) fn pending_producer(owner: Option<&Entry>, point: &OutPoint) -> bool {
    let index: u32 = point.index().into();
    owner.is_some_and(|owner| {
        owner.preaccepted() && (index as usize) < owner.transaction.outputs().len()
    })
}

fn db_available(snapshot: &Snapshot, point: &OutPoint) -> bool {
    matches!(
        crate::util::block_offload(|| snapshot.cell(point, false)),
        CellStatus::Live(_)
    )
}

fn live_available(
    state: &AuthorityState,
    point: &OutPoint,
    db_cache: &HashMap<OutPoint, bool>,
) -> bool {
    if state
        .spenders
        .contains_key(&OutPointKey::from_out_point(point))
    {
        return false;
    }
    if let Some(owner) = state.owner(&hash_key(&point.tx_hash()))
        && owner.accepted().is_some()
    {
        let index: u32 = point.index().into();
        return (index as usize) < owner.transaction.outputs().len();
    }
    db_cache.get(point).copied().unwrap_or(false)
}

/// Process one dirty dependency page. Returns `Ok(None)` when no dirty wake
/// exists, or `Ok(Some(batch))` after committing transitions and advancing the page.
pub(super) fn commit_wake(
    store: &Store,
    cursor: &mut Option<OutPointKey>,
) -> Result<Option<Option<Arc<Batch>>>, Error> {
    let Some(WakePageSnapshot {
        view,
        snapshot,
        key,
        pass,
        after,
        entries,
    }) = store.wake_page(cursor)
    else {
        return Ok(None);
    };
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.maintenance.wake").entered();

    // Collect every dependency key referenced by this bounded wake page, filter
    // out pool-accepted producers under ONE short read lock, then probe RocksDB off-lock.
    let mut keys_to_probe: HashSet<OutPoint> = HashSet::new();
    keys_to_probe.insert(key.to_out_point());
    for entry in &entries {
        match &entry.phase {
            Phase::Waiting(keys) => keys_to_probe.extend(keys.iter().cloned()),
            Phase::Replaced(triggers) => keys_to_probe.extend(triggers.keys().iter().cloned()),
            _ => {}
        }
    }
    let db_points: Vec<OutPoint> = {
        let state = store.state_read();
        if state.view != view {
            return Err(Error::Stale);
        }
        keys_to_probe
            .into_iter()
            .filter(|point| {
                state
                    .owner(&hash_key(&point.tx_hash()))
                    .is_none_or(|owner| owner.accepted().is_none())
            })
            .collect()
    };
    let mut db_cache: HashMap<OutPoint, bool> = HashMap::with_capacity(db_points.len());
    for point in db_points {
        let live = db_available(&snapshot, &point);
        db_cache.insert(point, live);
    }

    let (_, batch) = store.commit_with(|tx| {
        if tx.state.view != view {
            return Err(Error::Stale);
        }
        let Some(row) = tx.state.waiters.get(&key) else {
            return Err(Error::Stale);
        };
        if row.wake.as_ref().is_none_or(|wake| wake.pass != pass) {
            return Err(Error::Stale);
        }

        let mut edits = Vec::new();
        let mut effects = Vec::new();
        for original in &entries {
            let hash = original.hash_bytes();
            let Some(entry) = tx.state.owner(&hash).cloned() else {
                continue;
            };
            // Only act on the exact owner observed in the wake page; if replaced
            // concurrently, let its own phase/pass govern it.
            if !Arc::ptr_eq(&entry, original) {
                return Err(Error::Stale);
            }
            match &entry.phase {
                Phase::Waiting(keys) => {
                    let mut all_ready = true;
                    let mut lost = None;
                    for point in keys {
                        if live_available(tx.state, point, &db_cache) {
                            continue;
                        }
                        all_ready = false;
                        if !entry.source.requires_known_producer() {
                            break;
                        }
                        if !pending_producer(
                            tx.state.owner(&hash_key(&point.tx_hash())).map(Arc::as_ref),
                            point,
                        ) {
                            lost = Some(point.clone());
                            break;
                        }
                    }
                    if let Some(point) = lost {
                        effects.push(Effect::removed(
                            &entry,
                            Reject::Resolve(OutPointError::Unknown(point)),
                            None,
                        )?);
                        edits.push(OwnerEdit::remove(hash, entry));
                    } else if all_ready {
                        let next = entry.try_with_phase(Phase::Resolve)?;
                        edits.push(OwnerEdit::new(hash, Some(entry), Some(next)));
                    }
                }
                Phase::Replaced(triggers)
                    if triggers.is_ready(|dep| Ok(live_available(tx.state, dep, &db_cache)))? =>
                {
                    let next = Entry::try_new(
                        Arc::clone(&entry.transaction),
                        entry.arrival,
                        Source::Recovery,
                        Phase::Resolve,
                    )?;
                    edits.push(OwnerEdit::new(hash, Some(entry), Some(next)));
                }
                _ => {}
            }
        }

        tx.commit_edits(edits, Vec::new(), effects, Class::Trusted)?;
        tx.advance_wake(&key, pass, after);
        Ok(())
    })?;
    Ok(Some(batch))
}
