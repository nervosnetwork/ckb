extern crate rustc_hash;
extern crate slab;
use crate::component::pool_map::PoolMap;
use ckb_types::core::cell::{CellChecker, CellMetaBuilder, CellProvider, CellStatus};
use ckb_types::packed::{OutPoint, ProposalShortId};
use std::collections::HashSet;

pub(crate) struct PoolCell<'a> {
    pub pool_map: &'a PoolMap,
    observe_pool_spends: bool,
    excluded: Option<&'a HashSet<ProposalShortId>>,
}

impl<'a> PoolCell<'a> {
    /// Input view: accepted consumers make an outpoint dead unless the
    /// caller is doing the permissive first pass for RBF.
    pub fn for_inputs(pool_map: &'a PoolMap) -> Self {
        PoolCell {
            pool_map,
            observe_pool_spends: true,
            excluded: None,
        }
    }

    /// Permissive first-pass RBF input view. The final immutable pool plan
    /// rechecks inputs after excluding the complete replacement closure.
    pub fn for_rbf_inputs(pool_map: &'a PoolMap) -> Self {
        PoolCell {
            pool_map,
            observe_pool_spends: false,
            excluded: None,
        }
    }

    /// Cell-dependency view: an accepted consumer does not hide the
    /// pre-spend cell. The overlay can therefore fall through to the chain or
    /// return the accepted producer's output, independent of arrival order.
    pub fn for_dependencies(pool_map: &'a PoolMap) -> Self {
        PoolCell {
            pool_map,
            observe_pool_spends: false,
            excluded: None,
        }
    }

    /// Role-specific view over a virtual post-removal pool used by immutable
    /// mutation planning.
    pub fn excluding(
        pool_map: &'a PoolMap,
        observe_pool_spends: bool,
        excluded: &'a HashSet<ProposalShortId>,
    ) -> Self {
        Self {
            pool_map,
            observe_pool_spends,
            excluded: Some(excluded),
        }
    }

    fn is_excluded(&self, id: &ProposalShortId) -> bool {
        self.excluded.is_some_and(|excluded| excluded.contains(id))
    }
}

impl<'a> PoolCell<'a> {
    fn is_consumed_by_pool(&self, out_point: &OutPoint) -> bool {
        self.observe_pool_spends
            && self
                .pool_map
                .out_point_index
                .get_input_ref(out_point)
                .is_some_and(|owner| !self.is_excluded(owner))
    }
}

impl<'a> CellProvider for PoolCell<'a> {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        if self.is_consumed_by_pool(out_point) {
            return CellStatus::Dead;
        }
        if let Some(owner) = self.pool_map.get_by_hash(&out_point.tx_hash())
            && !self.is_excluded(&owner.id)
            && let Some((output, data)) = owner
                .inner
                .transaction()
                .output_with_data(out_point.index().into())
        {
            let cell_meta = CellMetaBuilder::from_cell_output(output, data)
                .out_point(out_point.to_owned())
                .build();
            CellStatus::live_cell(cell_meta)
        } else {
            CellStatus::Unknown
        }
    }
}

impl<'a> CellChecker for PoolCell<'a> {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        if self.is_consumed_by_pool(out_point) {
            return Some(false);
        }
        if self
            .pool_map
            .get_by_hash(&out_point.tx_hash())
            .filter(|owner| !self.is_excluded(&owner.id))
            .and_then(|owner| {
                owner
                    .inner
                    .transaction()
                    .output_with_data(out_point.index().into())
            })
            .is_some()
        {
            return Some(true);
        }
        None
    }
}
