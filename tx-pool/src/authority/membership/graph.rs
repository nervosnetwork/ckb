//! Tracked membership observations. Policy can read the graph and build its
//! borrowed Plan, but cannot replace the read set or access the raw Store.
use super::*;
use crate::authority::{
    budget::Limits,
    model::RelationKey,
    store::{CHILD, DEP, INPUT},
};

pub(in crate::authority) struct Graph<'a> {
    store: &'a Store,
    entries: Members,
    pub(super) plan: &'a mut Plan,
}
impl<'a> Graph<'a> {
    pub(in crate::authority) fn new(store: &'a Store, plan: &'a mut Plan) -> Self {
        Self {
            store,
            entries: Members::new(),
            plan,
        }
    }
    pub(super) fn entries(&self) -> &Members {
        &self.entries
    }
    pub(super) fn limits(&self) -> &Limits {
        &self.store.budget.limits
    }
    pub(super) fn accepted_usage(&self) -> Amount {
        self.store.budget.accepted_usage()
    }
    pub(super) fn spender(&mut self, point: &OutPoint) -> Result<Option<Byte32>, Error> {
        self.plan.spender(self.store, point)
    }
    /// Observe the complete input/dep relation, including an empty result.
    /// New readers of an output must be included when its producer is admitted.
    pub(super) fn readers(&mut self, point: OutPoint) -> Result<Vec<Byte32>, Error> {
        self.plan.members(
            self.store,
            &RelationKey::Dependency(DependencyKey::Cell(point)),
            INPUT | DEP,
        )
    }
    pub(super) fn capture_accepted(&mut self) -> Result<(), Error> {
        self.entries = self
            .plan
            .capture_accepted(self.store)?
            .into_iter()
            .map(|entry| (entry.hash(), entry))
            .collect();
        Ok(())
    }
    pub(in crate::authority) fn get(&mut self, hash: &Byte32) -> Result<Option<Arc<Entry>>, Error> {
        if let Some(entry) = self.entries.get(hash) {
            return Ok(Some(Arc::clone(entry)));
        }
        let entry = self
            .plan
            .original(self.store, hash)?
            .filter(|entry| entry.accepted().is_some());
        if let Some(entry) = &entry {
            self.entries.insert(compact_packed(hash), Arc::clone(entry));
        }
        Ok(entry)
    }
    pub(in crate::authority) fn require(&mut self, hash: &Byte32) -> Result<Arc<Entry>, Error> {
        self.get(hash)?.ok_or(Error::Stale)
    }
    pub(in crate::authority) fn descendants(
        &mut self,
        roots: impl IntoIterator<Item = Byte32>,
        removed: &BTreeSet<Byte32>,
        limit: usize,
    ) -> Result<BTreeSet<Byte32>, Error> {
        let mut result = BTreeSet::new();
        let mut stack: Vec<_> = roots.into_iter().collect();
        while let Some(hash) = stack.pop() {
            if removed.contains(&hash) || !result.insert(hash.clone()) {
                continue;
            }
            if result.len() > limit {
                return Err(component_limit(false));
            }
            self.require(&hash)?;
            stack.extend(
                self.plan
                    .members(self.store, &RelationKey::Children(hash), CHILD)?,
            );
        }
        Ok(result)
    }
    pub(in crate::authority) fn ancestors(
        &mut self,
        roots: impl IntoIterator<Item = Byte32>,
        removed: &BTreeSet<Byte32>,
        limit: usize,
    ) -> Result<BTreeSet<Byte32>, Error> {
        let mut result = BTreeSet::new();
        let mut stack: Vec<_> = roots.into_iter().collect();
        while let Some(hash) = stack.pop() {
            if removed.contains(&hash) || !result.insert(hash.clone()) {
                continue;
            }
            if result.len() > limit {
                return Err(Reject::ExceededMaximumAncestorsCount.into());
            }
            let entry = self.require(&hash)?;
            stack.extend(accepted(&entry)?.parents.iter().cloned());
        }
        Ok(result)
    }
    /// Compute original totals only for entries that need removal notices.
    /// Observe each descendant relation once, then reuse immutable parent edges.
    pub(in crate::authority) fn removal_totals(
        &mut self,
        hashes: &[Byte32],
        max_ancestors: usize,
    ) -> Result<BTreeMap<Byte32, (Aggregate, Aggregate)>, Error> {
        let mut totals = BTreeMap::new();
        for hash in hashes {
            let ancestors = self.ancestors([hash.clone()], &BTreeSet::new(), max_ancestors)?;
            totals.insert(
                compact_packed(hash),
                (aggregate(&self.entries, &ancestors)?, Aggregate::default()),
            );
        }
        let descendants = self.descendants(
            hashes.iter().cloned(),
            &BTreeSet::new(),
            self.store.budget.limits.accepted.items,
        )?;
        for hash in &descendants {
            let own = Aggregate::one(accepted(self.entries.get(hash).ok_or(Error::Stale)?)?);
            let mut seen = BTreeSet::new();
            let mut stack = vec![hash.clone()];
            while let Some(parent) = stack.pop() {
                if !descendants.contains(&parent) || !seen.insert(parent.clone()) {
                    continue;
                }
                if let Some((_, total)) = totals.get_mut(&parent) {
                    *total = total.add(own)?;
                }
                let entry = self.entries.get(&parent).ok_or(Error::Stale)?;
                stack.extend(accepted(entry)?.parents.iter().cloned());
            }
        }
        Ok(totals)
    }
    pub(in crate::authority) fn entry_snapshot(
        &mut self,
        hash: &Byte32,
        max_ancestors: usize,
    ) -> Result<TxEntrySnapshot, Error> {
        let entry = self.require(hash)?;
        let ancestors = self.ancestors([hash.clone()], &BTreeSet::new(), max_ancestors)?;
        let descendants = self.descendants(
            [hash.clone()],
            &BTreeSet::new(),
            self.store.budget.limits.accepted.items,
        )?;
        snapshot(
            &entry,
            aggregate(&self.entries, &ancestors)?,
            aggregate(&self.entries, &descendants)?,
        )
    }
}
