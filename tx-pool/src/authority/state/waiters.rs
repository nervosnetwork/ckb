//! Dependency waiter index and monotonic wake-pass pagination for
//! [`AuthorityState`].

use super::{AuthorityState, OwnerEdit};
use crate::authority::model::{Error, OutPointKey};
use ckb_types::packed::OutPoint;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
};

pub(in crate::authority) const WAKE_PAGE: usize = 32;

#[derive(Clone, Debug)]
pub(in crate::authority) struct WakeCursor {
    pub(in crate::authority) pass: u64,
    pub(in crate::authority) after: Option<[u8; 32]>,
}

/// Waiting and replacement-history entries watching one `OutPoint`.
#[derive(Debug, Default)]
pub(in crate::authority) struct WaitRow {
    /// Owner hash -> `wait_after_pass`. An entry can wake only when `pass > wait_after_pass`.
    pub(in crate::authority) waiters: BTreeMap<[u8; 32], u64>,
    pub(in crate::authority) wake: Option<WakeCursor>,
    pub(in crate::authority) next_pass: u64,
}

impl WaitRow {
    pub(in crate::authority) fn wake_candidates(
        &self,
        pass: u64,
        after: Option<&[u8; 32]>,
    ) -> impl Iterator<Item = &[u8; 32]> {
        self.waiters
            .range::<[u8; 32], _>((after.map_or(Unbounded, Excluded), Unbounded))
            .filter_map(move |(hash, wait_after)| (pass > *wait_after).then_some(hash))
    }

    pub(in crate::authority) fn is_empty(&self) -> bool {
        self.waiters.is_empty() && self.wake.is_none()
    }
}

pub(super) fn for_each_derived_wake_key(
    edits: &[OwnerEdit],
    mut visit: impl FnMut(OutPointKey) -> Result<(), Error>,
) -> Result<(), Error> {
    for edit in edits {
        if edit.after.is_none()
            && let Some(before) = edit.before.as_ref().filter(|e| e.preaccepted())
        {
            for idx in 0..before.transaction.outputs().len() {
                visit(OutPointKey::from_parts(edit.hash, idx as u32))?;
            }
        }
        for entry in edit
            .before
            .iter()
            .chain(&edit.after)
            .filter(|e| e.accepted().is_some())
        {
            for idx in 0..entry.transaction.outputs().len() {
                visit(OutPointKey::from_parts(edit.hash, idx as u32))?;
            }
            for point in entry.transaction.input_pts_iter() {
                visit(OutPointKey::from_out_point(&point))?;
            }
        }
    }
    Ok(())
}

impl AuthorityState {
    pub(super) fn add_waiter(
        &mut self,
        key: OutPointKey,
        hash: [u8; 32],
        defer_pass: bool,
    ) -> Result<(), Error> {
        let base_pass = self
            .waiters
            .get(&key)
            .map_or(self.wake_epoch, |row| row.next_pass);
        let wait_after_pass = base_pass
            .checked_add(u64::from(defer_pass))
            .ok_or(Error::Fault("wake pass"))?;
        if let Some(row) = self.waiters.get_mut(&key) {
            row.waiters.insert(hash, wait_after_pass);
        } else {
            self.waiters.insert(
                key,
                WaitRow {
                    waiters: BTreeMap::from([(hash, wait_after_pass)]),
                    wake: None,
                    next_pass: self.wake_epoch,
                },
            );
        }
        Ok(())
    }

    pub(super) fn remove_waiter_keys(&mut self, hash: &[u8; 32], keys: &BTreeSet<OutPoint>) {
        for key in keys {
            let point_key = OutPointKey::from_out_point(key);
            if let Some(row) = self.waiters.get_mut(&point_key) {
                row.waiters.remove(hash);
                if row.waiters.is_empty() && row.wake.take().is_some() {
                    self.dirty_wakes.remove(&point_key);
                }
                if row.is_empty() {
                    self.waiters.remove(&point_key);
                }
            }
        }
    }

    pub(in crate::authority) fn start_wake(&mut self, key: &OutPointKey) -> Result<bool, Error> {
        let Some(row) = self.waiters.get_mut(key).filter(|r| !r.waiters.is_empty()) else {
            return Ok(false);
        };
        let was_queued = row.wake.is_some();
        let pass = row
            .next_pass
            .checked_add(1)
            .ok_or(Error::Fault("wake pass"))?;
        let eligible = row.waiters.values().any(|wait_after| pass > *wait_after);
        // Consume even a deferred event so the next event is eligible.
        row.next_pass = pass;
        self.wake_epoch = self.wake_epoch.max(pass);
        if !eligible {
            return Ok(false);
        }
        row.wake = Some(WakeCursor { pass, after: None });
        if !was_queued {
            self.dirty_wakes.insert(*key);
        }
        Ok(true)
    }

    pub(in crate::authority) fn advance_wake(
        &mut self,
        key: &OutPointKey,
        pass: u64,
        after: Option<[u8; 32]>,
    ) {
        let Some(row) = self
            .waiters
            .get_mut(key)
            .filter(|r| r.wake.as_ref().is_some_and(|w| w.pass == pass))
        else {
            return;
        };
        if row.wake_candidates(pass, after.as_ref()).next().is_some() {
            row.wake = Some(WakeCursor { pass, after });
        } else {
            row.wake = None;
            self.dirty_wakes.remove(key);
        }
        if row.is_empty() {
            self.waiters.remove(key);
        }
        self.mutation_epoch = self.mutation_epoch.wrapping_add(1);
        self.structural_epoch = self.structural_epoch.wrapping_add(1);
        self.recent_leaf_inserts.clear();
    }
}
