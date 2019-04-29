use crate::block::Block;
use crate::transaction::{CellOutput, OutPoint, Transaction};
use crate::Capacity;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::iter::Chain;
use std::slice;

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum LiveCell {
    Null,
    Output(Box<CellMeta>),
}

#[derive(Clone, Eq, PartialEq, Debug, Default, Deserialize, Serialize)]
pub struct CellMeta {
    #[serde(skip)]
    pub cell_output: Option<CellOutput>,
    pub out_point: OutPoint,
    pub block_number: Option<u64>,
    pub cellbase: bool,
    pub capacity: Capacity,
    pub data_hash: Option<H256>,
}

impl From<&CellOutput> for CellMeta {
    fn from(output: &CellOutput) -> Self {
        CellMeta {
            cell_output: Some(output.clone()),
            capacity: output.capacity,
            ..Default::default()
        }
    }
}

impl CellMeta {
    pub fn is_cellbase(&self) -> bool {
        self.cellbase
    }

    pub fn capacity(&self) -> Capacity {
        self.capacity
    }

    pub fn data_hash(&self) -> Option<&H256> {
        self.data_hash.as_ref()
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum CellStatus {
    /// Cell exists and has not been spent.
    Live(LiveCell),
    /// Cell exists and has been spent.
    Dead,
    /// Cell does not exist.
    Unknown,
}

impl CellStatus {
    pub fn live_null() -> CellStatus {
        CellStatus::Live(LiveCell::Null)
    }

    pub fn live_cell(cell_meta: CellMeta) -> CellStatus {
        CellStatus::Live(LiveCell::Output(Box::new(cell_meta)))
    }

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

    pub fn get_live_cell(&self) -> Option<&CellMeta> {
        match *self {
            CellStatus::Live(LiveCell::Output(ref cell_meta)) => Some(cell_meta),
            _ => None,
        }
    }

    pub fn take_live_cell(self) -> Option<CellMeta> {
        match self {
            CellStatus::Live(LiveCell::Output(cell_meta)) => Some(*cell_meta),
            _ => None,
        }
    }
}

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction<'a> {
    pub transaction: &'a Transaction,
    pub dep_cells: Vec<CellStatus>,
    pub input_cells: Vec<CellStatus>,
}

pub trait CellProvider {
    fn cell(&self, out_point: &OutPoint) -> CellStatus;

    fn get_cell_status(&self, out_point: &OutPoint) -> CellStatus {
        if out_point.is_null() {
            CellStatus::Live(LiveCell::Null)
        } else {
            self.cell(out_point)
        }
    }
}

pub struct OverlayCellProvider<'a> {
    overlay: &'a CellProvider,
    cell_provider: &'a CellProvider,
}

impl<'a> OverlayCellProvider<'a> {
    pub fn new(overlay: &'a CellProvider, cell_provider: &'a CellProvider) -> Self {
        Self {
            overlay,
            cell_provider,
        }
    }
}

impl<'a> CellProvider for OverlayCellProvider<'a> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        match self.overlay.get_cell_status(out_point) {
            CellStatus::Live(co) => CellStatus::Live(co),
            CellStatus::Dead => CellStatus::Dead,
            CellStatus::Unknown => self.cell_provider.get_cell_status(out_point),
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
            .transactions()
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
        match self.output_indices.get(&out_point.tx_hash).and_then(|i| {
            self.block.transactions()[*i]
                .outputs()
                .get(out_point.index as usize)
        }) {
            Some(output) => CellStatus::live_cell(CellMeta {
                cell_output: Some(output.clone()),
                out_point: out_point.to_owned(),
                data_hash: None,
                capacity: output.capacity,
                block_number: Some(self.block.header().number()),
                cellbase: out_point.index == 0,
            }),
            None => CellStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnresolvableError {
    Dead(Vec<OutPoint>),
    Unknown(Vec<OutPoint>),
}

impl std::error::Error for UnresolvableError {}

impl std::fmt::Display for UnresolvableError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self, f)
    }
}

pub fn resolve_transaction<'a, CP: CellProvider>(
    transaction: &'a Transaction,
    seen_inputs: &mut FnvHashSet<OutPoint>,
    cell_provider: &CP,
) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
    let input_cells = transaction
        .input_pts()
        .iter()
        .map(|input| {
            if seen_inputs.insert(input.clone()) {
                cell_provider.get_cell_status(input)
            } else {
                CellStatus::Dead
            }
        })
        .collect();

    let dep_cells = transaction
        .dep_pts()
        .iter()
        .map(|dep| {
            if seen_inputs.contains(dep) {
                CellStatus::Dead
            } else {
                cell_provider.get_cell_status(dep)
            }
        })
        .collect();

    Ok(ResolvedTransaction {
        transaction,
        input_cells,
        dep_cells,
    })
}

impl<'a> ResolvedTransaction<'a> {
    pub fn cells_iter(&self) -> Chain<slice::Iter<CellStatus>, slice::Iter<CellStatus>> {
        self.dep_cells.iter().chain(&self.input_cells)
    }

    pub fn cells_iter_mut(
        &mut self,
    ) -> Chain<slice::IterMut<CellStatus>, slice::IterMut<CellStatus>> {
        self.dep_cells.iter_mut().chain(&mut self.input_cells)
    }

    pub fn is_double_spend(&self) -> bool {
        self.cells_iter().any(CellStatus::is_dead)
    }

    pub fn is_orphan(&self) -> bool {
        self.cells_iter().any(CellStatus::is_unknown)
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.cells_iter().all(CellStatus::is_live)
    }

    pub fn fee(&self) -> ::occupied_capacity::Result<Capacity> {
        self.inputs_capacity().and_then(|x| {
            self.transaction.outputs_capacity().and_then(|y| {
                if x > y {
                    x.safe_sub(y)
                } else {
                    Ok(Capacity::zero())
                }
            })
        })
    }

    pub fn inputs_capacity(&self) -> ::occupied_capacity::Result<Capacity> {
        self.input_cells
            .iter()
            .filter_map(|cell_status| {
                if let CellStatus::Live(LiveCell::Output(cell_meta)) = cell_status {
                    Some(cell_meta.capacity())
                } else {
                    None
                }
            })
            .try_fold(Capacity::zero(), Capacity::safe_add)
    }
}

#[cfg(test)]
mod tests {
    use super::super::script::Script;
    use super::*;
    use crate::{capacity_bytes, Bytes, Capacity};
    use numext_fixed_hash::H256;
    use std::collections::HashMap;

    struct CellMemoryDb {
        cells: HashMap<OutPoint, Option<CellMeta>>,
    }
    impl CellProvider for CellMemoryDb {
        fn cell(&self, o: &OutPoint) -> CellStatus {
            match self.cells.get(o) {
                Some(&Some(ref cell_meta)) => {
                    CellStatus::Live(LiveCell::Output(Box::new(cell_meta.clone())))
                }
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
            tx_hash: H256::zero(),
            index: 1,
        };
        let p2 = OutPoint {
            tx_hash: H256::zero(),
            index: 2,
        };
        let p3 = OutPoint {
            tx_hash: H256::zero(),
            index: 3,
        };
        let o = {
            let cell_output = CellOutput {
                capacity: capacity_bytes!(2),
                data: Bytes::default(),
                lock: Script::default(),
                type_: None,
            };
            CellMeta {
                block_number: Some(1),
                capacity: cell_output.capacity,
                data_hash: Some(cell_output.data_hash()),
                cell_output: Some(cell_output),
                out_point: OutPoint {
                    tx_hash: Default::default(),
                    index: 0,
                },
                cellbase: false,
            }
        };

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(
            CellStatus::Live(LiveCell::Output(Box::new(o))),
            db.get_cell_status(&p1)
        );
        assert_eq!(CellStatus::Dead, db.get_cell_status(&p2));
        assert_eq!(CellStatus::Unknown, db.get_cell_status(&p3));
    }
}
