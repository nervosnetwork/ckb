extern crate rustc_hash;
extern crate slab;
use crate::component::pool_map::PoolMap;
use ckb_types::core::cell::{CellChecker, CellMeta, CellMetaBuilder, CellProvider, CellStatus};
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
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

/// Whether resolved chain metadata was produced against the exact chain tip
/// used by final admission.
///
/// This is intentionally a closed enum instead of a boolean parameter: only
/// [`Self::from_tips`] can establish the positive-evidence arm.
#[derive(Clone, Copy)]
enum TxPoolChainEvidence {
    SameTip,
    Revalidate,
}

impl TxPoolChainEvidence {
    fn from_tips(resolved: &Byte32, current: &Byte32) -> Self {
        if resolved == current {
            Self::SameTip
        } else {
            Self::Revalidate
        }
    }
}

/// Tx-pool-only, role-aware final-admission checker for a resolved transaction.
///
/// The accepted-pool overlay always wins because pool spends/producers can
/// change after resolution. On an overlay miss, a `CellMeta` carrying chain
/// `transaction_info` is already a positive liveness receipt for the same
/// immutable tip; stale tips and pool-produced metadata still fall through to
/// the chain checker. This removes duplicate RocksDB reads without adding a
/// cache, invalidation protocol, lock or second state authority.
///
/// Block and consensus validation must not use this checker: they resolve
/// against their own validation context and retain `CellChecker`'s conservative
/// default behavior.
pub(crate) struct TxPoolResolvedCellChecker<'a, A, B> {
    overlay: &'a A,
    chain: &'a B,
    evidence: TxPoolChainEvidence,
}

impl<'a, A, B> TxPoolResolvedCellChecker<'a, A, B> {
    pub(crate) fn new(
        overlay: &'a A,
        chain: &'a B,
        resolved_tip: &Byte32,
        current_tip: &Byte32,
    ) -> Self {
        Self {
            overlay,
            chain,
            evidence: TxPoolChainEvidence::from_tips(resolved_tip, current_tip),
        }
    }
}

impl<A: CellChecker, B: CellChecker> CellChecker for TxPoolResolvedCellChecker<'_, A, B> {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        self.overlay
            .is_live(out_point)
            .or_else(|| self.chain.is_live(out_point))
    }

    fn is_live_resolved_cell(&self, cell: &CellMeta) -> Option<bool> {
        self.overlay.is_live(&cell.out_point).or_else(|| {
            if matches!(self.evidence, TxPoolChainEvidence::SameTip)
                && cell.transaction_info.is_some()
            {
                Some(true)
            } else {
                self.chain.is_live(&cell.out_point)
            }
        })
    }
}

#[cfg(test)]
#[path = "tests/pool_cell.rs"]
mod tests;
