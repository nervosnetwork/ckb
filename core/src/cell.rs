use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::iter::Chain;
use std::slice;
use transaction::{CellOutput, OutPoint, Transaction};

#[derive(Clone, PartialEq, Debug)]
pub enum CellStatus {
    /// Cell exists and has not been spent.
    Current(CellOutput),
    /// Cell exists and has been spent.
    Old,
    /// Cell does not exist.
    Unknown,
}

impl CellStatus {
    pub fn is_current(&self) -> bool {
        match *self {
            CellStatus::Current(_) => true,
            _ => false,
        }
    }

    pub fn is_old(&self) -> bool {
        self == &CellStatus::Old
    }

    pub fn is_unknown(&self) -> bool {
        self == &CellStatus::Unknown
    }

    pub fn get_current(&self) -> Option<&CellOutput> {
        match *self {
            CellStatus::Current(ref output) => Some(output),
            _ => None,
        }
    }

    pub fn take_current(self) -> Option<CellOutput> {
        match self {
            CellStatus::Current(output) => Some(output),
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

    fn cell_at(&self, out_point: &OutPoint, parent: &H256) -> CellStatus;

    fn resolve_transaction(&self, transaction: &Transaction) -> ResolvedTransaction {
        let mut seen_inputs = HashSet::new();

        let input_cells = transaction
            .input_pts()
            .iter()
            .map(|input| {
                if seen_inputs.insert(input.clone()) {
                    self.cell(input)
                } else {
                    CellStatus::Old
                }
            })
            .collect();

        let dep_cells = transaction
            .dep_pts()
            .iter()
            .map(|dep| {
                if seen_inputs.insert(dep.clone()) {
                    self.cell(dep)
                } else {
                    CellStatus::Old
                }
            })
            .collect();

        ResolvedTransaction {
            transaction: transaction.clone(),
            input_cells,
            dep_cells,
        }
    }

    fn resolve_transaction_at(
        &self,
        transaction: &Transaction,
        parent: &H256,
    ) -> ResolvedTransaction {
        let mut seen_inputs = HashSet::new();

        let input_cells = transaction
            .input_pts()
            .iter()
            .map(|input| {
                if seen_inputs.insert(input.clone()) {
                    self.cell_at(input, parent)
                } else {
                    CellStatus::Old
                }
            })
            .collect();

        let dep_cells = transaction
            .dep_pts()
            .iter()
            .map(|dep| {
                if seen_inputs.insert(dep.clone()) {
                    self.cell_at(dep, parent)
                } else {
                    CellStatus::Old
                }
            })
            .collect();

        ResolvedTransaction {
            transaction: transaction.clone(),
            input_cells,
            dep_cells,
        }
    }

    fn resolve_transaction_unknown_inputs(&self, resolved_transaction: &mut ResolvedTransaction) {
        for (out_point, state) in resolved_transaction.transaction.out_points_iter().zip(
            resolved_transaction
                .dep_cells
                .iter_mut()
                .chain(&mut resolved_transaction.input_cells),
        ) {
            if *state == CellStatus::Unknown {
                *state = self.cell(out_point);
            }
        }
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
        self.cells_iter().any(|state| state.is_old())
    }

    pub fn is_orphan(&self) -> bool {
        self.cells_iter().any(|state| state.is_unknown())
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.cells_iter().all(|state| state.is_current())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use numext_fixed_hash::H256;
    use std::collections::HashMap;

    struct CellMemoryDb {
        cells: HashMap<OutPoint, Option<CellOutput>>,
    }
    impl CellProvider for CellMemoryDb {
        fn cell(&self, o: &OutPoint) -> CellStatus {
            match self.cells.get(o) {
                Some(&Some(ref cell_output)) => CellStatus::Current(cell_output.clone()),
                Some(&None) => CellStatus::Old,
                None => CellStatus::Unknown,
            }
        }

        fn cell_at(&self, o: &OutPoint, _: &H256) -> CellStatus {
            match self.cells.get(o) {
                Some(&Some(ref cell_output)) => CellStatus::Current(cell_output.clone()),
                Some(&None) => CellStatus::Old,
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
        let o = CellOutput {
            capacity: 2,
            data: vec![],
            lock: H256::default(),
            contract: None,
        };

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(CellStatus::Current(o), db.cell(&p1));
        assert_eq!(CellStatus::Old, db.cell(&p2));
        assert_eq!(CellStatus::Unknown, db.cell(&p3));
    }
}
