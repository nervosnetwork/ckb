//! Tracked membership observations. Policy can read the graph and build its
//! borrowed Plan, but cannot replace the read set or access the raw Store.
use super::*;
use crate::authority::budget::Limits;

pub(in crate::authority) struct Graph<'a> {
    store: &'a Store,
    // Keys reuse the cached hashes already retained by their owners.
    observed: Members,
    pub(super) plan: &'a mut Plan,
}
impl<'a> Graph<'a> {
    pub(in crate::authority) fn new(store: &'a Store, plan: &'a mut Plan) -> Self {
        Self {
            store,
            observed: Members::new(),
            plan,
        }
    }

    /// Entries reached by this decision; only a capacity trim captures all accepted entries.
    pub(super) fn observed(&self) -> &Members {
        &self.observed
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
        self.plan.readers(self.store, point)
    }

    /// Capture the full accepted graph and its charge, preserving earlier premises.
    pub(super) fn capture_accepted(&mut self) -> Result<Amount, Error> {
        let (members, charge) = self.plan.capture_accepted(self.store)?;
        self.observed = members;
        Ok(charge)
    }

    /// Inserting a new leaf adds it once to each ancestor's descendant family.
    /// Other families retain their original edges. A round visits only bounds
    /// that can beat its best exact rank, excluding earlier victims.
    pub(super) fn certified_evictions(
        &mut self,
        candidate: &AdmissionCandidate,
        added: Amount,
        snapshot: &Snapshot,
    ) -> Result<Option<BTreeSet<Byte32>>, Error> {
        let (bounds, usage) = self.store.eviction_bounds(self.plan)?;
        let mut excess = usage
            .checked_add(added)
            .ok_or_else(overflow)?
            .positive_difference(self.limits().accepted);
        let mut removed = BTreeSet::new();
        if self.get(candidate.entry.transaction.hash_ref())?.is_some() {
            return Ok(None);
        }
        let own = Aggregate::one(accepted(&candidate.entry)?);
        let candidate_rank = EvictionRank::new(&candidate.entry, own, snapshot)?;
        let mut work = MAX_POOL_MUTATION_CANDIDATES;
        while excess != Amount::default() {
            let mut winner = candidate_rank.clone();
            let mut selected = None;
            for bound in EvictionRank::ordered_bounds(&bounds) {
                if bound >= &winner {
                    break;
                }
                if work == 0 {
                    return Ok(None);
                }
                if removed.contains(&bound.hash) {
                    work = work.saturating_sub(1);
                    continue;
                }
                let family = match self.descendants([bound.hash.clone()], &removed, work) {
                    Ok(family) => family,
                    Err(Error::Rejected(Reject::Full(_))) => return Ok(None),
                    Err(error) => return Err(error),
                };
                work = work.saturating_sub(family.len());
                let root = self.require(&bound.hash)?;
                let protected = candidate.ancestors.contains(&bound.hash);
                let mut total = aggregate(&self.observed, &family)?;
                if protected {
                    total = total.add(own)?;
                }
                let rank = EvictionRank::new(&root, total, snapshot)?;
                if rank < winner {
                    winner = rank;
                    selected = (!protected).then_some(family);
                }
            }
            let family = selected.ok_or_else(|| protected_eviction(candidate_rank.fee))?;
            for hash in &family {
                excess = excess.positive_difference(owner_amount(self.require(hash)?.as_ref())?);
            }
            removed.extend(family);
        }
        Ok(Some(removed))
    }

    pub(in crate::authority) fn get(&mut self, hash: &Byte32) -> Result<Option<Arc<Entry>>, Error> {
        if let Some(entry) = self.observed.get(hash) {
            return Ok(Some(Arc::clone(entry)));
        }
        let entry = self
            .plan
            .original(self.store, hash)?
            .filter(|entry| entry.accepted().is_some());
        if let Some(entry) = &entry {
            self.observed.insert(entry.hash(), Arc::clone(entry));
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
                return Err(component_limit());
            }
            self.require(&hash)?;
            stack.extend(self.plan.children(self.store, hash)?);
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
    ) -> Result<BTreeMap<Byte32, NeighborhoodTotals>, Error> {
        let mut totals = BTreeMap::new();
        for hash in hashes {
            let ancestors = self.ancestors([hash.clone()], &BTreeSet::new(), max_ancestors)?;
            totals.insert(
                compact_packed(hash),
                NeighborhoodTotals {
                    ancestors: aggregate(&self.observed, &ancestors)?,
                    descendants: Aggregate::default(),
                },
            );
        }
        let descendants = self.descendants(
            hashes.iter().cloned(),
            &BTreeSet::new(),
            self.store.budget.limits.accepted.items,
        )?;
        for hash in &descendants {
            let own = Aggregate::one(accepted(
                self.observed
                    .get(hash)
                    .ok_or(Error::Fault("captured descendant"))?,
            )?);
            let mut seen = BTreeSet::new();
            let mut stack = vec![hash.clone()];
            while let Some(parent) = stack.pop() {
                if !descendants.contains(&parent) || !seen.insert(parent.clone()) {
                    continue;
                }
                if let Some(total) = totals.get_mut(&parent) {
                    total.descendants = total.descendants.add(own)?;
                }
                let entry = self
                    .observed
                    .get(&parent)
                    .ok_or(Error::Fault("captured ancestor"))?;
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
        super::entry_snapshot(
            &entry,
            NeighborhoodTotals {
                ancestors: aggregate(&self.observed, &ancestors)?,
                descendants: aggregate(&self.observed, &descendants)?,
            },
        )
    }
}
