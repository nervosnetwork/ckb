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
    plan: &mut Plan,
) -> Result<bool, Error> {
    use ckb_types::core::cell::{CellProvider, CellStatus, HeaderChecker};
    match key {
        DependencyKey::Header(hash) => {
            Ok(crate::util::block_offload(|| snapshot.check_valid(hash)).is_ok())
        }
        DependencyKey::Cell(point) => {
            if plan.spender(store, point)?.is_some() {
                return Ok(false);
            }
            if let Some(owner) = plan.get(store, &point.tx_hash())?
                && owner.accepted().is_some()
            {
                let index: u32 = point.index().into();
                return Ok((index as usize) < owner.transaction.outputs().len());
            }
            Ok(matches!(
                crate::util::block_offload(|| snapshot.cell(point, false)),
                CellStatus::Live(_)
            ))
        }
    }
}
/// A trusted missing input can wait only for a known, not-yet-accepted
/// producer with that exact output. Absence is terminal on its current cut.
pub(super) fn pending_producer(owner: Option<&Entry>, point: &ckb_types::packed::OutPoint) -> bool {
    let index: u32 = point.index().into();
    owner.is_some_and(|owner| {
        owner.preaccepted() && (index as usize) < owner.transaction.outputs().len()
    })
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
    let mut plan = Plan::new(view, Class::Trusted, Default::default());
    // The page shares one trigger. Its first observation stays in the Plan
    // and is validated for every waiter when the whole plan commits.
    let mut trigger_ready = None;
    let mut ready = |key: &DependencyKey, plan: &mut Plan| {
        if key != &page.key {
            return available(store, &snapshot, key, plan);
        }
        if let Some(ready) = trigger_ready {
            return Ok(ready);
        }
        let ready = available(store, &snapshot, key, plan)?;
        trigger_ready = Some(ready);
        Ok(ready)
    };
    for hash in &page.hashes {
        let Some(entry) = plan.get(store, hash)? else {
            continue;
        };
        match &entry.phase {
            Phase::Waiting(keys) => {
                let mut all_ready = true;
                let mut lost = None;
                for key in keys {
                    if ready(key, &mut plan)? {
                        continue;
                    }
                    all_ready = false;
                    if !entry.source.requires_known_producer() {
                        break;
                    }
                    if let DependencyKey::Cell(point) = key
                        && !pending_producer(plan.get(store, &point.tx_hash())?.as_deref(), point)
                    {
                        lost = Some(point.clone());
                        break;
                    }
                    // Trusted waiting must still find a terminal missing
                    // producer on later keys, even if this one can wait.
                }
                if let Some(point) = lost {
                    let effect = Effect::removed(
                        &entry,
                        Reject::Resolve(OutPointError::Unknown(point)),
                        None,
                    )?;
                    plan.edit(Some(entry), None, Some(effect))?;
                } else if all_ready {
                    let after = entry.with_phase(Phase::Resolve);
                    plan.edit(Some(entry), Some(after), None)?;
                }
            }
            Phase::Replaced(triggers) if triggers.is_ready(|key| ready(key, &mut plan))? => {
                let after = Arc::new(Entry {
                    transaction: Arc::clone(&entry.transaction),
                    arrival: entry.arrival,
                    source: Source::Recovery,
                    phase: Phase::Resolve,
                });
                plan.edit(Some(entry), Some(after), None)?;
            }
            _ => {}
        }
    }
    plan.advance(page);
    Ok(Some(plan))
}
