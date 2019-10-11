use crate::component::container::SortedTxMap;
use ckb_types::packed::ProposalShortId;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Relation {
    Ancestor,
    Descendant,
}

/// This crate is used for recursively searching in-pool ancestors or descendants
pub(crate) trait RelativeTraversal {
    fn get_relative_ids(
        &self,
        id: &ProposalShortId,
        relation: Relation,
    ) -> HashSet<ProposalShortId>;

    fn get_parents(&self, id: &ProposalShortId) -> &HashSet<ProposalShortId>;

    fn get_children(&self, id: &ProposalShortId) -> &HashSet<ProposalShortId>;

    fn get_ancestors(&self, id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.get_relative_ids(id, Relation::Ancestor)
    }

    fn get_descendants(&self, id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.get_relative_ids(id, Relation::Descendant)
    }
}

impl RelativeTraversal for SortedTxMap {
    fn get_parents(&self, id: &ProposalShortId) -> &HashSet<ProposalShortId> {
        &self
            .get_link(id)
            .expect("found inconsistency when get_parents")
            .parents
    }

    fn get_children(&self, id: &ProposalShortId) -> &HashSet<ProposalShortId> {
        &self
            .get_link(id)
            .expect("found inconsistency when get_children")
            .children
    }

    fn get_relative_ids(
        &self,
        id: &ProposalShortId,
        relation: Relation,
    ) -> HashSet<ProposalShortId> {
        let mut queue = get_direct_ids(self, id, relation).clone();
        let mut relatives = HashSet::with_capacity(queue.len());
        while !queue.is_empty() {
            let relative_id = queue.iter().next().expect("checked above").clone();
            queue.remove(&relative_id);
            relatives.insert(relative_id.clone());

            // Recursively search the next relative entries
            for direct_id in get_direct_ids(self, &relative_id, relation) {
                if !relatives.contains(direct_id) {
                    queue.insert(direct_id.clone());
                }
            }
        }

        relatives
    }
}

fn get_direct_ids<'a, T: RelativeTraversal>(
    container: &'a T,
    id: &ProposalShortId,
    relation: Relation,
) -> &'a HashSet<ProposalShortId> {
    match relation {
        Relation::Ancestor => container.get_parents(id),
        Relation::Descendant => container.get_children(id),
    }
}
