use crate::block::Block;
use crate::transaction::{CellOutput, OutPoint, Transaction};
use crate::Capacity;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use std::iter::Chain;
use std::slice;

#[derive(Clone, PartialEq, Debug)]
pub struct CellMeta {
    pub cell_output: CellOutput,
    pub block_number: Option<u64>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum CellStatus {
    /// Cell exists and has not been spent.
    Live(CellMeta),
    /// Cell exists and has been spent.
    Dead,
    /// Cell does not exist.
    Unknown,
}

impl CellStatus {
    pub fn is_live(&self) -> bool {
        match *self {
            CellStatus::Live(_) => true,
            _ => false,
        }
    }

    pub fn is_dead(&self) -> bool {
        self == &CellStatus::Dead
    }

    pub fn is_unknown(&self) -> bool {
        self == &CellStatus::Unknown
    }

    pub fn get_live(&self) -> Option<&CellMeta> {
        match *self {
            CellStatus::Live(ref output) => Some(output),
            _ => None,
        }
    }

    pub fn take_live(self) -> Option<CellMeta> {
        match self {
            CellStatus::Live(output) => Some(output),
            _ => None,
        }
    }
}

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction {
    pub transaction: Transaction,
    pub dep_cells: Vec<CellStatus>,
    pub input_cells: Vec<CellStatus>,
}

pub trait CellProvider {
    fn cell(&self, out_point: &OutPoint) -> CellStatus;
}

pub struct OverlayCellProvider<'a, O, CP> {
    overlay: &'a O,
    cell_provider: &'a CP,
}

impl<'a, O, CP> OverlayCellProvider<'a, O, CP> {
    pub fn new(overlay: &'a O, cell_provider: &'a CP) -> Self {
        OverlayCellProvider {
            overlay,
            cell_provider,
        }
    }
}

impl<'a, O, CP> CellProvider for OverlayCellProvider<'a, O, CP>
where
    O: CellProvider,
    CP: CellProvider,
{
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        match self.overlay.cell(out_point) {
            CellStatus::Live(co) => CellStatus::Live(co),
            CellStatus::Dead => CellStatus::Dead,
            CellStatus::Unknown => self.cell_provider.cell(out_point),
        }
    }
}

pub struct BlockCellProvider<'a> {
    output_indices: FnvHashMap<H256, usize>,
    block: &'a Block,
}

impl<'a> BlockCellProvider<'a> {
    pub fn new(block: &'a Block) -> Self {
        let output_indices = block
            .commit_transactions()
            .iter()
            .enumerate()
            .map(|(idx, tx)| (tx.hash(), idx))
            .collect();
        Self {
            output_indices,
            block,
        }
    }
}

impl<'a> CellProvider for BlockCellProvider<'a> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if let Some(i) = self.output_indices.get(&out_point.hash) {
            match self.block.commit_transactions()[*i]
                .outputs()
                .get(out_point.index as usize)
            {
                Some(x) => CellStatus::Live(CellMeta {
                    cell_output: x.clone(),
                    block_number: Some(self.block.header().number()),
                }),
                None => CellStatus::Unknown,
            }
        } else {
            CellStatus::Unknown
        }
    }
}

pub fn resolve_transaction<CP: CellProvider>(
    transaction: &Transaction,
    seen_inputs: &mut FnvHashSet<OutPoint>,
    cell_provider: &CP,
) -> ResolvedTransaction {
    let input_cells = transaction
        .input_pts()
        .iter()
        .map(|input| {
            if seen_inputs.insert(input.clone()) {
                cell_provider.cell(input)
            } else {
                CellStatus::Dead
            }
        })
        .collect();

    let dep_cells = transaction
        .dep_pts()
        .iter()
        .map(|dep| {
            if seen_inputs.insert(dep.clone()) {
                cell_provider.cell(dep)
            } else {
                CellStatus::Dead
            }
        })
        .collect();

    ResolvedTransaction {
        transaction: transaction.clone(),
        input_cells,
        dep_cells,
    }
}

impl ResolvedTransaction {
    pub fn cells_iter(&self) -> Chain<slice::Iter<CellStatus>, slice::Iter<CellStatus>> {
        self.dep_cells.iter().chain(&self.input_cells)
    }

    pub fn cells_iter_mut(
        &mut self,
    ) -> Chain<slice::IterMut<CellStatus>, slice::IterMut<CellStatus>> {
        self.dep_cells.iter_mut().chain(&mut self.input_cells)
    }

    pub fn is_double_spend(&self) -> bool {
        self.cells_iter().any(|state| state.is_dead())
    }

    pub fn is_orphan(&self) -> bool {
        self.cells_iter().any(|state| state.is_unknown())
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.cells_iter().all(|state| state.is_live())
    }

    pub fn fee(&self) -> Capacity {
        self.inputs_capacity()
            .saturating_sub(self.transaction.outputs_capacity())
    }

    pub fn inputs_capacity(&self) -> Capacity {
        self.input_cells
            .iter()
            .filter_map(|cell_status| {
                if let CellStatus::Live(cell_meta) = cell_status {
                    Some(cell_meta.cell_output.capacity)
                } else {
                    None
                }
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::super::script::Script;
    use super::*;
    use numext_fixed_hash::H256;
    use std::collections::HashMap;

    struct CellMemoryDb {
        cells: HashMap<OutPoint, Option<CellMeta>>,
    }
    impl CellProvider for CellMemoryDb {
        fn cell(&self, o: &OutPoint) -> CellStatus {
            match self.cells.get(o) {
                Some(&Some(ref cell)) => CellStatus::Live(cell.clone()),
                Some(&None) => CellStatus::Dead,
                None => CellStatus::Unknown,
            }
        }
    }

    #[test]
    fn cell_provider_trait_works() {
        let mut db = CellMemoryDb {
            cells: HashMap::new(),
        };

        let p1 = OutPoint {
            hash: H256::zero(),
            index: 1,
        };
        let p2 = OutPoint {
            hash: H256::zero(),
            index: 2,
        };
        let p3 = OutPoint {
            hash: H256::zero(),
            index: 3,
        };
        let o = CellMeta {
            block_number: Some(1),
            cell_output: CellOutput {
                capacity: 2,
                data: vec![],
                lock: Script::default(),
                type_: None,
            },
        };

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(CellStatus::Live(o), db.cell(&p1));
        assert_eq!(CellStatus::Dead, db.cell(&p2));
        assert_eq!(CellStatus::Unknown, db.cell(&p3));
    }
}
