use crate::block::Block;
use crate::transaction::{CellOutput, OutPoint, Transaction};
use crate::Capacity;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

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

#[derive(PartialEq, Debug)]
pub enum CellStatus {
    /// Cell exists and has not been spent.
    Live(Box<CellMeta>),
    /// Cell exists and has been spent.
    Dead,
    /// Cell does not exist.
    Unknown,
}

impl CellStatus {
    pub fn live_cell(cell_meta: CellMeta) -> CellStatus {
        CellStatus::Live(Box::new(cell_meta))
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
}

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction<'a> {
    pub transaction: &'a Transaction,
    pub dep_cells: Vec<CellMeta>,
    pub input_cells: Vec<CellMeta>,
}

pub trait CellProvider {
    fn cell(&self, out_point: &OutPoint) -> CellStatus;
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
        match self.overlay.cell(out_point) {
            CellStatus::Live(cell_meta) => CellStatus::Live(cell_meta),
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UnresolvableError {
    Dead(OutPoint),
    Unknown(Vec<OutPoint>),
}

pub fn resolve_transaction<'a, CP: CellProvider>(
    transaction: &'a Transaction,
    seen_inputs: &mut FnvHashSet<OutPoint>,
    cell_provider: &CP,
) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
    let (mut unknown_out_points, mut input_cells, mut dep_cells) = (
        Vec::new(),
        Vec::with_capacity(transaction.inputs().len()),
        Vec::with_capacity(transaction.deps().len()),
    );

    // skip resolve input of cellbase
    if !transaction.is_cellbase() {
        for out_point in transaction.input_pts() {
            let cell_status = if seen_inputs.insert(out_point.clone()) {
                cell_provider.cell(&out_point)
            } else {
                CellStatus::Dead
            };

            match cell_status {
                CellStatus::Dead => {
                    return Err(UnresolvableError::Dead(out_point.clone()));
                }
                CellStatus::Unknown => {
                    unknown_out_points.push(out_point.clone());
                }
                CellStatus::Live(cell_meta) => {
                    input_cells.push(*cell_meta);
                }
            }
        }
    }

    for out_point in transaction.dep_pts() {
        let cell_status = if seen_inputs.contains(&out_point) {
            CellStatus::Dead
        } else {
            cell_provider.cell(&out_point)
        };

        match cell_status {
            CellStatus::Dead => {
                return Err(UnresolvableError::Dead(out_point.clone()));
            }
            CellStatus::Unknown => {
                unknown_out_points.push(out_point.clone());
            }
            CellStatus::Live(cell_meta) => {
                dep_cells.push(*cell_meta);
            }
        }
    }

    if !unknown_out_points.is_empty() {
        Err(UnresolvableError::Unknown(unknown_out_points))
    } else {
        Ok(ResolvedTransaction {
            transaction,
            input_cells,
            dep_cells,
        })
    }
}

impl<'a> ResolvedTransaction<'a> {
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
            .map(CellMeta::capacity)
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
                Some(&Some(ref cell_meta)) => CellStatus::live_cell(cell_meta.clone()),
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

        assert_eq!(CellStatus::Live(Box::new(o)), db.cell(&p1));
        assert_eq!(CellStatus::Dead, db.cell(&p2));
        assert_eq!(CellStatus::Unknown, db.cell(&p3));
    }
}
