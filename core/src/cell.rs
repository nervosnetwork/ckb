use crate::transaction::{CellOutput, OutPoint, Transaction};
use fnv::FnvHashSet;
use std::iter::Chain;
use std::slice;

#[derive(Clone, PartialEq, Debug)]
pub enum CellStatus {
    /// Cell exists and has not been spent.
    Live(CellOutput),
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

    pub fn get_live(&self) -> Option<&CellOutput> {
        match *self {
            CellStatus::Live(ref output) => Some(output),
            _ => None,
        }
    }

    pub fn take_live(self) -> Option<CellOutput> {
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

    fn cell_at<F: Fn(&OutPoint) -> Option<bool>>(
        &self,
        out_point: &OutPoint,
        is_spent: F,
    ) -> CellStatus;

    fn resolve_transaction(&self, transaction: &Transaction) -> ResolvedTransaction {
        let mut seen_inputs = FnvHashSet::default();
        resolve_transaction(transaction, &mut seen_inputs, |x| self.cell(x))
    }

    fn resolve_transaction_at<F: Fn(&OutPoint) -> Option<bool>>(
        &self,
        transaction: &Transaction,
        is_spent: F,
    ) -> ResolvedTransaction {
        let mut seen_inputs = FnvHashSet::default();
        resolve_transaction(transaction, &mut seen_inputs, |x| {
            self.cell_at(x, |o| is_spent(o))
        })
    }

    fn resolve_transaction_unknown_inputs(&self, resolved_transaction: &mut ResolvedTransaction) {
        resolve_transaction_unknown_inputs(resolved_transaction, |x| self.cell(x))
    }
}

pub fn resolve_transaction<F: Fn(&OutPoint) -> CellStatus>(
    transaction: &Transaction,
    seen_inputs: &mut FnvHashSet<OutPoint>,
    cell: F,
) -> ResolvedTransaction {
    let input_cells = transaction
        .input_pts()
        .iter()
        .map(|input| {
            if seen_inputs.insert(input.clone()) {
                cell(input)
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
                cell(dep)
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

pub fn resolve_transaction_unknown_inputs<F: Fn(&OutPoint) -> CellStatus>(
    resolved_transaction: &mut ResolvedTransaction,
    cell: F,
) {
    for (out_point, state) in resolved_transaction.transaction.out_points_iter().zip(
        resolved_transaction
            .dep_cells
            .iter_mut()
            .chain(&mut resolved_transaction.input_cells),
    ) {
        if *state == CellStatus::Unknown {
            *state = cell(out_point);
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
        self.cells_iter().any(|state| state.is_dead())
    }

    pub fn is_orphan(&self) -> bool {
        self.cells_iter().any(|state| state.is_unknown())
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.cells_iter().all(|state| state.is_live())
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
                Some(&Some(ref cell_output)) => CellStatus::Live(cell_output.clone()),
                Some(&None) => CellStatus::Dead,
                None => CellStatus::Unknown,
            }
        }

        fn cell_at<F: Fn(&OutPoint) -> Option<bool>>(&self, o: &OutPoint, _: F) -> CellStatus {
            match self.cells.get(o) {
                Some(&Some(ref cell_output)) => CellStatus::Live(cell_output.clone()),
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
        let o = CellOutput {
            capacity: 2,
            data: vec![],
            lock: H256::default(),
            type_: None,
        };

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(CellStatus::Live(o), db.cell(&p1));
        assert_eq!(CellStatus::Dead, db.cell(&p2));
        assert_eq!(CellStatus::Unknown, db.cell(&p3));
    }
}
