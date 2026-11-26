//! Accepted transaction graph traversal and incremental O(1) descendant
//! closure aggregate maintenance for [`AuthorityState`].

use super::{AuthorityState, FastHashMap, FastHashSet, HashKeyBuildHasher, OwnerEdit};
use crate::authority::state::ReaderSet;
use crate::authority::{
    membership::{Aggregate, NeighborhoodTotals},
    model::{Entry, Error, hash_key},
};
use std::{
    collections::{HashSet, hash_map::Entry as MapEntry},
    sync::Arc,
};

pub(super) struct DescendantsUpdate {
    added: Vec<[u8; 32]>,
    recompute: FastHashSet<[u8; 32]>,
}

impl AuthorityState {
    fn accepted_children(&self, hash: &[u8; 32]) -> Result<Option<&ReaderSet>, Error> {
        if self
            .owners
            .get(hash)
            .is_none_or(|entry| entry.accepted().is_none())
        {
            return Err(Error::Fault("accepted descendant projection"));
        }
        Ok(self.children.get(hash))
    }

    /// Core DFS over accepted descendants of `roots` (excluding `excluded`) up
    /// to `limit` nodes. Returns `Ok(false)` if the closure exceeds `limit`.
    pub(super) fn visit_descendants(
        &self,
        roots: impl IntoIterator<Item = [u8; 32]>,
        excluded: Option<&HashSet<[u8; 32]>>,
        limit: usize,
        mut on_visit: impl FnMut([u8; 32]),
    ) -> Result<bool, Error> {
        let mut seen = FastHashSet::default();
        let mut stack: Vec<[u8; 32]> = roots.into_iter().collect();
        while let Some(hash) = stack.pop() {
            if excluded.is_some_and(|ex| ex.contains(&hash)) || !seen.insert(hash) {
                continue;
            }
            if seen.len() > limit {
                return Ok(false);
            }
            let children = self.accepted_children(&hash)?;
            on_visit(hash);
            if let Some(children) = children {
                for child in children.iter() {
                    if !excluded.is_some_and(|ex| ex.contains(child)) && !seen.contains(child) {
                        stack.push(*child);
                    }
                }
            }
        }
        Ok(true)
    }

    /// Bounded descendant closure of `root`, excluding `excluded`.
    pub(in crate::authority) fn descendants_bounded(
        &self,
        root: &[u8; 32],
        excluded: &HashSet<[u8; 32]>,
        limit: usize,
    ) -> Result<Option<Vec<[u8; 32]>>, Error> {
        let mut order = Vec::new();
        let ok = self.visit_descendants([*root], Some(excluded), limit, |h| order.push(h))?;
        Ok(ok.then_some(order))
    }

    /// Unbounded descendant closure of `root`, excluding `excluded`.
    pub(in crate::authority) fn descendants_unbounded(
        &self,
        root: &[u8; 32],
        excluded: &HashSet<[u8; 32]>,
    ) -> Result<HashSet<[u8; 32]>, Error> {
        self.descendants_of_roots([*root], excluded, usize::MAX)?
            .ok_or(Error::Fault("unbounded descendants"))
    }

    /// Return the exact live descendant closure aggregate (including `hash`
    /// itself) for an accepted owner in O(1).
    pub(in crate::authority) fn descendant_aggregate(
        &self,
        hash: &[u8; 32],
    ) -> Result<Aggregate, Error> {
        if let Some(&aggregate) = self.descendants.get(hash) {
            return Ok(aggregate);
        }
        let entry = self.owners.get(hash).ok_or(Error::Stale)?;
        Ok(Aggregate::one(entry.accepted().ok_or(Error::Stale)?))
    }

    /// Collect all strict live ancestors of an accepted owner (`|anc| <= max_ancestors_count`).
    pub(in crate::authority) fn strict_ancestors(&self, hash: &[u8; 32]) -> FastHashSet<[u8; 32]> {
        let mut ancestors = FastHashSet::default();
        let Some(accepted) = self.owners.get(hash).and_then(|e| e.accepted()) else {
            return ancestors;
        };
        if accepted.parents.is_empty() {
            return ancestors;
        }
        let mut stack: Vec<[u8; 32]> = accepted.parents.iter().map(hash_key).collect();
        while let Some(parent) = stack.pop() {
            if parent == *hash || ancestors.contains(&parent) {
                continue;
            }
            if let Some(parent_accepted) = self.owners.get(&parent).and_then(|e| e.accepted()) {
                ancestors.insert(parent);
                stack.extend(parent_accepted.parents.iter().map(hash_key));
            }
        }
        ancestors
    }

    /// Count strict accepted descendants of an accepted `root` in O(1).
    pub(in crate::authority) fn descendant_count(&self, root: &[u8; 32]) -> Result<usize, Error> {
        if self.owners.get(root).is_none_or(|e| e.accepted().is_none()) {
            return Err(Error::Stale);
        }
        Ok(self
            .descendants
            .get(root)
            .map_or(0, |aggregate| aggregate.count.saturating_sub(1)))
    }

    /// Bounded descendant closure of multiple `roots`, excluding `excluded`.
    pub(in crate::authority) fn descendants_of_roots(
        &self,
        roots: impl IntoIterator<Item = [u8; 32]>,
        excluded: &HashSet<[u8; 32]>,
        limit: usize,
    ) -> Result<Option<HashSet<[u8; 32]>>, Error> {
        let mut out = HashSet::new();
        let ok = self.visit_descendants(roots, Some(excluded), limit, |h| {
            out.insert(h);
        })?;
        Ok(ok.then_some(out))
    }

    /// Capture one accepted descendant closure starting at `hash` (root first).
    pub(in crate::authority) fn capture_descendants(
        &self,
        hash: &[u8; 32],
    ) -> Result<Vec<Arc<Entry>>, Error> {
        if self.owners.get(hash).is_none_or(|e| e.accepted().is_none()) {
            return Ok(Vec::new());
        }
        let mut owners = Vec::new();
        self.visit_descendants([*hash], None, usize::MAX, |h| {
            if let Some(entry) = self.owners.get(&h) {
                owners.push(Arc::clone(entry));
            }
        })?;
        Ok(owners)
    }

    /// Compute exact ancestor & descendant aggregates for accepted entries
    /// scheduled for removal, using the live state before mutation.
    pub(in crate::authority) fn removal_totals(
        &self,
        hashes: &[[u8; 32]],
        max_ancestors: usize,
    ) -> Result<FastHashMap<[u8; 32], NeighborhoodTotals>, Error> {
        let mut totals = FastHashMap::with_capacity_and_hasher(hashes.len(), HashKeyBuildHasher);
        let mut seen = FastHashSet::default();
        let mut stack = Vec::new();
        for hash in hashes {
            let entry = self
                .owners
                .get(hash)
                .ok_or(Error::Fault("removal total owner"))?;
            let own = entry.accepted().ok_or(Error::Stale)?;
            let ancestors = if own.parents.is_empty() {
                if max_ancestors == 0 {
                    return Err(crate::error::Reject::ExceededMaximumAncestorsCount.into());
                }
                Aggregate::one(own)
            } else {
                seen.clear();
                stack.clear();
                seen.insert(*hash);
                let mut sum = Aggregate::one(own);
                stack.extend(own.parents.iter().map(hash_key));
                while let Some(parent) = stack.pop() {
                    if parent == *hash {
                        return Err(crate::error::Reject::Invalidated(format!(
                            "candidate would create a causal cycle through {:?}",
                            entry.transaction.hash_ref()
                        ))
                        .into());
                    }
                    if !seen.insert(parent) {
                        continue;
                    }
                    if seen.len() > max_ancestors {
                        return Err(crate::error::Reject::ExceededMaximumAncestorsCount.into());
                    }
                    let value = self
                        .owners
                        .get(&parent)
                        .and_then(|e| e.accepted())
                        .ok_or(Error::Stale)?;
                    sum = sum.add(Aggregate::one(value))?;
                    stack.extend(value.parents.iter().map(hash_key));
                }
                if seen.len() > max_ancestors {
                    return Err(crate::error::Reject::ExceededMaximumAncestorsCount.into());
                }
                sum
            };
            let descendants = self.descendant_aggregate(hash)?;
            totals.insert(
                *hash,
                NeighborhoodTotals {
                    ancestors,
                    descendants,
                },
            );
        }
        Ok(totals)
    }

    fn subtract_descendant(&mut self, ancestor: &[u8; 32], own: Aggregate) -> Result<(), Error> {
        let MapEntry::Occupied(mut slot) = self.descendants.entry(*ancestor) else {
            return Err(Error::Fault("missing descendant aggregate"));
        };
        let next = slot.get().checked_sub(own)?;
        if next.count <= 1 {
            slot.remove();
        } else {
            *slot.get_mut() = next;
        }
        Ok(())
    }

    fn add_descendant(&mut self, ancestor: [u8; 32], own: Aggregate) -> Result<(), Error> {
        match self.descendants.entry(ancestor) {
            MapEntry::Occupied(mut slot) => {
                *slot.get_mut() = slot.get().add(own)?;
            }
            MapEntry::Vacant(slot) => {
                let base = Aggregate::one(
                    self.owners
                        .get(&ancestor)
                        .and_then(|e| e.accepted())
                        .ok_or(Error::Fault("descendant ancestor owner"))?,
                );
                slot.insert(base.add(own)?);
            }
        }
        Ok(())
    }

    pub(super) fn prepare_descendants_update(
        &mut self,
        edit_slice: &[OwnerEdit],
    ) -> Result<Option<DescendantsUpdate>, Error> {
        if let [edit] = edit_slice {
            if let (Some(old), Some(new)) = (&edit.before, &edit.after)
                && Arc::ptr_eq(old, new)
            {
                return Ok(None);
            }
            match (
                edit.before.as_deref().and_then(Entry::accepted),
                edit.after.as_deref().and_then(Entry::accepted),
            ) {
                (None, None) => return Ok(None),
                (None, Some(after)) => {
                    if after.parents.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(DescendantsUpdate {
                        added: vec![edit.hash],
                        recompute: FastHashSet::default(),
                    }));
                }
                (Some(before), None) if !self.children.contains_key(&edit.hash) => {
                    if !before.parents.is_empty() {
                        let own = Aggregate::one(before);
                        for anc in self.strict_ancestors(&edit.hash) {
                            self.subtract_descendant(&anc, own)?;
                        }
                    }
                    self.descendants.remove(&edit.hash);
                    return Ok(None);
                }
                _ => {}
            }
        }

        let mut removed_accepted = HashSet::new();
        let mut added = Vec::new();
        let mut surviving_edits = Vec::new();
        for edit in edit_slice {
            if let (Some(old), Some(new)) = (&edit.before, &edit.after)
                && Arc::ptr_eq(old, new)
            {
                continue;
            }
            match (
                edit.before.as_deref().and_then(Entry::accepted),
                edit.after.as_deref().and_then(Entry::accepted),
            ) {
                (None, None) => {}
                (Some(_), None) => {
                    removed_accepted.insert(edit.hash);
                }
                (None, Some(_)) => {
                    added.push(edit.hash);
                }
                (Some(old), Some(new)) => {
                    surviving_edits.push((edit.hash, old, new));
                }
            }
        }
        if removed_accepted.is_empty() && added.is_empty() && surviving_edits.is_empty() {
            return Ok(None);
        }

        let mut dirty_parent_roots = Vec::new();
        let mut weight_changed = Vec::new();
        for &(hash, old, new) in &surviving_edits {
            let old_own = Aggregate::one(old);
            let new_own = Aggregate::one(new);
            if old_own != new_own {
                weight_changed.push((hash, old_own, new_own));
            }
            if old.parents != new.parents {
                let added_parent = new.parents.iter().any(|p| !old.parents.contains(p));
                let removed_live_ancestor = old.parents.iter().any(|p| {
                    if new.parents.contains(p) {
                        return false;
                    }
                    let key = hash_key(p);
                    !removed_accepted.contains(&key)
                        || self
                            .strict_ancestors(&key)
                            .iter()
                            .any(|anc| !removed_accepted.contains(anc))
                });
                if added_parent || removed_live_ancestor {
                    dirty_parent_roots.push(hash);
                }
            }
        }

        let mut recompute = FastHashSet::default();
        if !dirty_parent_roots.is_empty() {
            self.visit_descendants(
                dirty_parent_roots,
                Some(&removed_accepted),
                usize::MAX,
                |hash| {
                    recompute.insert(hash);
                },
            )?;
        }
        for &(hash, _, _) in &weight_changed {
            recompute.insert(hash);
        }

        for hash in removed_accepted.iter().chain(recompute.iter()) {
            let old_acc = self
                .owners
                .get(hash)
                .and_then(|e| e.accepted())
                .ok_or(Error::Fault("descendant old owner"))?;
            if !old_acc.parents.is_empty() {
                let own = Aggregate::one(old_acc);
                for anc in self.strict_ancestors(hash) {
                    if !removed_accepted.contains(&anc) {
                        self.subtract_descendant(&anc, own)?;
                    }
                }
            }
        }
        for hash in &removed_accepted {
            self.descendants.remove(hash);
        }
        for (hash, old_own, new_own) in weight_changed {
            if let Some(agg) = self.descendants.get_mut(&hash) {
                *agg = agg.checked_sub(old_own)?.add(new_own)?;
            }
        }

        if added.is_empty() && recompute.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DescendantsUpdate { added, recompute }))
        }
    }

    pub(super) fn finish_descendants_update(
        &mut self,
        update: DescendantsUpdate,
    ) -> Result<(), Error> {
        for hash in update.added.into_iter().chain(update.recompute) {
            let new_acc = self
                .owners
                .get(&hash)
                .and_then(|e| e.accepted())
                .ok_or(Error::Fault("descendant new owner"))?;
            if !new_acc.parents.is_empty() {
                let own = Aggregate::one(new_acc);
                for anc in self.strict_ancestors(&hash) {
                    self.add_descendant(anc, own)?;
                }
            }
        }
        Ok(())
    }
}
