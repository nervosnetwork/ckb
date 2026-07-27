use super::{EntryOutPointEdges, PoolEntry, PoolMap, Relation, Status, TxLinks};
use crate::component::entry::TxEntry;
use crate::error::Reject;
use ckb_types::core::Cycle;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::{HashMap, HashSet};

type Graph = HashMap<ProposalShortId, HashSet<ProposalShortId>>;

fn checked_sum(
    current: (usize, usize, Cycle, u64),
    entry: &crate::component::entry::TxEntry,
) -> Result<(usize, usize, Cycle, u64), String> {
    Ok((
        current
            .0
            .checked_add(1)
            .ok_or_else(|| "relationship count overflow".to_string())?,
        current
            .1
            .checked_add(entry.size)
            .ok_or_else(|| "relationship size overflow".to_string())?,
        current
            .2
            .checked_add(entry.cycles)
            .ok_or_else(|| "relationship cycles overflow".to_string())?,
        current
            .3
            .checked_add(entry.fee.as_u64())
            .ok_or_else(|| "relationship fee overflow".to_string())?,
    ))
}

fn transitive_closure(
    root: &ProposalShortId,
    graph: &Graph,
) -> Result<HashSet<ProposalShortId>, String> {
    let mut closure = HashSet::new();
    let mut stack = graph
        .get(root)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<_>>();
    while let Some(id) = stack.pop() {
        if &id == root {
            return Err(format!("dependency cycle reaches {root}"));
        }
        if !graph.contains_key(&id) {
            return Err(format!("relationship references missing entry {id}"));
        }
        if closure.insert(id.clone())
            && let Some(next) = graph.get(&id)
        {
            stack.extend(next.iter().cloned());
        }
    }
    Ok(closure)
}

impl PoolMap {
    /// Independently reconstruct every derived accepted-pool invariant from
    /// transaction membership. This is deliberately test-only: it is an
    /// exhaustive proof oracle, not hot-path production bookkeeping.
    pub(crate) fn audit(&self) -> Result<(), String> {
        let entries: HashMap<ProposalShortId, &PoolEntry> = self
            .entries
            .iter()
            .map(|(_, entry)| (entry.id.clone(), entry))
            .collect();
        if entries.len() != self.entries.len() {
            return Err("duplicate proposal id in authoritative entries".to_string());
        }

        let mut hash_to_id = HashMap::<Byte32, ProposalShortId>::with_capacity(entries.len());
        let mut expected_inputs = HashMap::<OutPoint, ProposalShortId>::new();
        let mut expected_deps = HashMap::<OutPoint, HashSet<ProposalShortId>>::new();
        let mut expected_headers = HashMap::<ProposalShortId, Vec<Byte32>>::new();
        let mut status_counts = (0usize, 0usize, 0usize);
        let mut totals = (0usize, 0usize, 0 as Cycle);

        for (id, entry) in &entries {
            if entry.inner.proposal_short_id() != (*id).clone() {
                return Err(format!("entry key differs from transaction short id {id}"));
            }
            if entry.hash != entry.inner.transaction().hash() {
                return Err(format!("entry full-hash key differs from transaction {id}"));
            }
            if hash_to_id
                .insert(entry.inner.transaction().hash(), (*id).clone())
                .is_some()
            {
                return Err("duplicate full transaction hash in pool".to_string());
            }
            if entry.score != entry.inner.as_score_key()
                || entry.evict_key != entry.inner.as_evict_key()
            {
                return Err(format!("derived sort key mismatch for {id}"));
            }
            match entry.status {
                Status::Pending => status_counts.0 += 1,
                Status::Gap => status_counts.1 += 1,
                Status::Proposed => status_counts.2 += 1,
            }
            totals = (
                totals
                    .0
                    .checked_add(entry.inner.size)
                    .ok_or_else(|| "total serialized size overflow".to_string())?,
                totals
                    .1
                    .checked_add(entry.inner.resident_size())
                    .ok_or_else(|| "total resident size overflow".to_string())?,
                totals
                    .2
                    .checked_add(entry.inner.cycles)
                    .ok_or_else(|| "total cycles overflow".to_string())?,
            );

            let inputs: HashSet<_> = entry.inner.transaction().input_pts_iter().collect();
            for input in &inputs {
                if let Some(other) = expected_inputs.insert(input.clone(), (*id).clone()) {
                    return Err(format!("input {input:?} is owned by both {other} and {id}"));
                }
            }
            for dep in entry.inner.related_dep_out_points() {
                if !inputs.contains(dep) {
                    expected_deps
                        .entry(dep.clone())
                        .or_default()
                        .insert((*id).clone());
                }
            }
            let headers = entry
                .inner
                .transaction()
                .header_deps()
                .into_iter()
                .collect::<Vec<_>>();
            if !headers.is_empty() {
                expected_headers.insert((*id).clone(), headers);
            }
        }

        if expected_inputs != self.out_point_index.inputs {
            return Err("accepted input index mismatch".to_string());
        }
        if expected_deps != self.out_point_index.deps {
            return Err("accepted dep index mismatch".to_string());
        }
        if expected_headers != self.out_point_index.header_deps {
            return Err("accepted header-dep index mismatch".to_string());
        }
        if status_counts
            != (
                self.stats.pending_count,
                self.stats.gap_count,
                self.stats.proposed_count,
            )
        {
            return Err("accepted status count mismatch".to_string());
        }
        if totals
            != (
                self.stats.total_tx_size,
                self.stats.total_tx_resident_size,
                self.stats.total_tx_cycles,
            )
        {
            return Err("accepted total accounting mismatch".to_string());
        }

        let mut expected_parents: Graph = entries
            .keys()
            .cloned()
            .map(|id| (id, HashSet::new()))
            .collect();
        for (id, entry) in &entries {
            let parents = expected_parents
                .get_mut(id)
                .expect("every entry initialized a graph node");
            for input in entry.inner.transaction().input_pts_iter() {
                if let Some(parent) = hash_to_id.get(&input.tx_hash()) {
                    parents.insert(parent.clone());
                }
            }
            for dep in entry.inner.related_dep_out_points() {
                if let Some(parent) = hash_to_id.get(&dep.tx_hash()) {
                    parents.insert(parent.clone());
                }
            }
        }
        let mut expected_children: Graph = entries
            .keys()
            .cloned()
            .map(|id| (id, HashSet::new()))
            .collect();
        for (child, parents) in &expected_parents {
            for parent in parents {
                expected_children
                    .get_mut(parent)
                    .ok_or_else(|| format!("missing parent entry {parent}"))?
                    .insert(child.clone());
            }
        }

        let actual_links: HashMap<_, _> = self
            .links
            .iter()
            .map(|(id, links)| (id.clone(), (links.parents.clone(), links.children.clone())))
            .collect();
        if actual_links.len() != entries.len() {
            return Err("links contain a missing or ghost node".to_string());
        }
        for id in entries.keys() {
            let Some((parents, children)) = actual_links.get(id) else {
                return Err(format!("entry {id} has no links node"));
            };
            if parents != &expected_parents[id] || children != &expected_children[id] {
                return Err(format!("direct relationship mismatch for {id}"));
            }
        }

        for (id, entry) in entries {
            let ancestors = transitive_closure(&id, &expected_parents)?;
            let descendants = transitive_closure(&id, &expected_children)?;
            // The entry itself consumes one ancestor slot. Express the
            // boundary without arithmetic so an impossible `usize` overflow
            // cannot turn a corrupt graph into a passing audit.
            if ancestors.len() >= self.max_ancestors_count {
                return Err(format!("ancestor limit exceeded by {id}"));
            }
            let mut ancestor_weight = checked_sum((0, 0, 0, 0), &entry.inner)?;
            for ancestor in ancestors {
                ancestor_weight = checked_sum(
                    ancestor_weight,
                    &entries_for(&self.entries, &ancestor)?.inner,
                )?;
            }
            let mut descendant_weight = checked_sum((0, 0, 0, 0), &entry.inner)?;
            for descendant in descendants {
                descendant_weight = checked_sum(
                    descendant_weight,
                    &entries_for(&self.entries, &descendant)?.inner,
                )?;
            }
            if ancestor_weight
                != (
                    entry.inner.ancestors_count,
                    entry.inner.ancestors_size,
                    entry.inner.ancestors_cycles,
                    entry.inner.ancestors_fee.as_u64(),
                )
            {
                return Err(format!("ancestor weight mismatch for {id}"));
            }
            if descendant_weight
                != (
                    entry.inner.descendants_count,
                    entry.inner.descendants_size,
                    entry.inner.descendants_cycles,
                    entry.inner.descendants_fee.as_u64(),
                )
            {
                return Err(format!("descendant weight mismatch for {id}"));
            }
        }
        Ok(())
    }
}

fn entries_for<'a>(
    entries: &'a super::MultiIndexPoolEntryMap,
    id: &ProposalShortId,
) -> Result<&'a PoolEntry, String> {
    entries
        .get_by_id(id)
        .ok_or_else(|| format!("relationship references absent entry {id}"))
}

impl PoolMap {
    pub(crate) fn header_deps_len(&self) -> usize {
        self.out_point_index.header_deps_len()
    }

    pub(crate) fn deps_len(&self) -> usize {
        self.out_point_index.deps_len()
    }

    pub(crate) fn inputs_len(&self) -> usize {
        self.out_point_index.inputs_len()
    }

    pub(crate) fn size(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.get_by_id(id).is_some()
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.entries
            .get_by_id(id)
            .map(|entry| entry.inner.transaction())
    }

    /// Permissive graph constructor used only by component fixtures that must
    /// build historical child-first states. Production and internal plug
    /// paths use immutable Plan/total Apply and cannot bypass its policy.
    pub(crate) fn add_entry(&mut self, mut entry: TxEntry, status: Status) -> Result<bool, Reject> {
        let id = entry.proposal_short_id();
        let hash = entry.transaction().hash();
        if self.entries.get_by_hash(&hash).is_some() {
            return Ok(false);
        }
        if self.entries.get_by_id(&id).is_some() {
            return Err(Reject::Full(format!(
                "proposal short-id collision while inserting {hash}"
            )));
        }

        let edges = EntryOutPointEdges::from_entry(&entry);
        self.fixture_pre_validate_entry_inputs(&edges)?;
        self.prevalidate_add_totals(&entry)?;
        let parents = self.prepare_ancestors_for_test(&mut entry)?;
        self.fixture_commit_ancestor_links(id.clone(), parents);
        self.fixture_record_entry_edges(&entry, &edges);
        let descendants = self.link_existing_children_for_test(&mut entry, &id);
        self.fixture_insert_entry(&entry, status);
        self.fixture_add_to_descendants(&entry, &descendants);
        self.fixture_add_to_ancestors(&entry);
        match status {
            Status::Pending => self.stats.pending_count += 1,
            Status::Gap => self.stats.gap_count += 1,
            Status::Proposed => self.stats.proposed_count += 1,
        }
        self.publish_stats_metrics();
        self.stats.total_tx_size += entry.size;
        self.stats.total_tx_resident_size += entry.resident_size();
        self.stats.total_tx_cycles += entry.cycles;
        Ok(true)
    }

    fn fixture_pre_validate_entry_inputs(&self, edges: &EntryOutPointEdges) -> Result<(), Reject> {
        for input in &edges.inputs {
            if self.out_point_index.get_input_ref(input).is_some() {
                return Err(Reject::Resolve(
                    ckb_types::core::error::OutPointError::Dead(input.clone()),
                ));
            }
        }
        Ok(())
    }

    fn fixture_commit_ancestor_links(
        &mut self,
        short_id: ProposalShortId,
        parents: HashSet<ProposalShortId>,
    ) {
        for parent in &parents {
            assert_eq!(self.links.add_child(parent, short_id.clone()), Some(true));
        }
        self.links.add_link(
            short_id,
            TxLinks {
                parents,
                children: HashSet::new(),
            },
        );
    }

    fn fixture_record_entry_edges(&mut self, entry: &TxEntry, edges: &EntryOutPointEdges) {
        let id = entry.proposal_short_id();
        for input in &edges.inputs {
            self.out_point_index
                .insert_input(input.clone(), id.clone())
                .unwrap();
        }
        for dep in &edges.deps {
            self.out_point_index.insert_deps(dep.clone(), id.clone());
        }
        let headers = entry.transaction().header_deps();
        if !headers.is_empty() {
            self.out_point_index
                .header_deps
                .insert(id, headers.into_iter().collect());
        }
    }

    fn fixture_insert_entry(&mut self, entry: &TxEntry, status: Status) {
        self.entries
            .try_insert(PoolEntry {
                hash: entry.transaction().hash(),
                id: entry.proposal_short_id(),
                score: entry.as_score_key(),
                status,
                evict_key: entry.as_evict_key(),
                inner: entry.clone(),
            })
            .unwrap();
    }

    fn fixture_add_to_descendants(
        &mut self,
        parent: &TxEntry,
        descendants: &HashSet<ProposalShortId>,
    ) {
        for id in descendants {
            self.entries
                .modify_by_id(id, |entry| {
                    entry.inner.add_ancestor_weight(parent).unwrap();
                    entry.score = entry.inner.as_score_key();
                })
                .unwrap();
        }
    }

    fn fixture_add_to_ancestors(&mut self, child: &TxEntry) {
        for id in self.links.calc_ancestors(&child.proposal_short_id()) {
            self.entries
                .modify_by_id(&id, |entry| {
                    entry.inner.add_descendant_weight(child).unwrap();
                    entry.evict_key = entry.inner.as_evict_key();
                })
                .unwrap();
        }
    }

    fn prepare_ancestors_for_test(
        &self,
        entry: &mut TxEntry,
    ) -> Result<HashSet<ProposalShortId>, Reject> {
        let parents = self
            .get_tx_parents(entry, self.max_ancestors_count.saturating_sub(1))
            .ok_or(Reject::ExceededMaximumAncestorsCount)?;
        let ancestors = self
            .links
            .calc_relation_ids(parents.clone(), Relation::Parents);
        if ancestors.len() >= self.max_ancestors_count {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }
        self.apply_ancestor_weights(entry, &ancestors)
            .map_err(|fault| Reject::Internal(format!("fixture ancestor fault: {fault:?}")))?;
        Ok(parents)
    }

    fn link_existing_children_for_test(
        &mut self,
        entry: &mut TxEntry,
        id: &ProposalShortId,
    ) -> HashSet<ProposalShortId> {
        let mut children = HashSet::new();
        for output in entry.transaction().output_pts() {
            if let Some(readers) = self.out_point_index.get_deps_ref(&output) {
                children.extend(readers.iter().cloned());
            }
            if let Some(spender) = self.out_point_index.get_input_ref(&output) {
                children.insert(spender.clone());
            }
        }
        for child in &children {
            assert!(self.links.add_parent(child, id.clone()).unwrap());
        }
        self.links
            .get_mut(id)
            .expect("test parent links were committed")
            .children
            .extend(children);
        let descendants = self.links.calc_descendants(id);
        for descendant in &descendants {
            entry
                .add_descendant_weight(&self.get_by_id(descendant).unwrap().inner)
                .unwrap();
        }
        descendants
    }

    fn prevalidate_add_totals(&self, entry: &TxEntry) -> Result<(), Reject> {
        let overflow = self.stats.total_tx_size.checked_add(entry.size).is_none()
            || self
                .stats
                .total_tx_resident_size
                .checked_add(entry.resident_size())
                .is_none()
            || self
                .stats
                .total_tx_cycles
                .checked_add(entry.cycles)
                .is_none();
        if overflow {
            Err(Reject::Full(
                "tx-pool test fixture totals overflow".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn add_proposed(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        self.add_entry(entry, Status::Proposed)
    }

    pub(crate) fn get_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
    ) -> HashSet<ProposalShortId> {
        self.score_sorted_iter_by_status(Status::Pending)
            .filter_map(|entry| {
                let id = entry.proposal_short_id();
                (!exclusion.contains(&id)).then_some(id)
            })
            .take(limit)
            .collect()
    }
}
