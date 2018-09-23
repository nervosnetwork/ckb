use bigint::H256;
use std::collections::HashSet;
use std::iter::Chain;
use std::slice;
use transaction::{CellOutput, OutPoint, Transaction};

#[derive(Clone, PartialEq, Debug)]
pub enum CellState {
    /// Cell exists and has not been spent.
    Head(CellOutput),
    /// Cell exists and has been spent.
    Tail,
    /// Cell does not exist.
    Unknown,
}

impl CellState {
    pub fn is_head(&self) -> bool {
        if let CellState::Head(_) = *self {
            true
        } else {
            false
        }
    }

    pub fn is_tail(&self) -> bool {
        *self == CellState::Tail
    }

    pub fn is_unknown(&self) -> bool {
        *self == CellState::Unknown
    }

    pub fn head(&self) -> Option<&CellOutput> {
        match *self {
            CellState::Head(ref output) => Some(output),
            _ => None,
        }
    }

    pub fn take_head(self) -> Option<CellOutput> {
        match self {
            CellState::Head(output) => Some(output),
            _ => None,
        }
    }
}

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction {
    pub transaction: Transaction,
    pub dep_cells: Vec<CellState>,
    pub input_cells: Vec<CellState>,
}

pub trait CellProvider {
    fn cell(&self, out_point: &OutPoint) -> CellState;

    fn cell_at(&self, out_point: &OutPoint, parent: &H256) -> CellState;

    fn resolve_transaction(&self, transaction: &Transaction) -> ResolvedTransaction {
        let mut seen_inputs = HashSet::new();

        let input_cells = transaction
            .input_pts()
            .iter()
            .map(|input| {
                if seen_inputs.insert(*input) {
                    self.cell(input)
                } else {
                    CellState::Tail
                }
            }).collect();

        let dep_cells = transaction
            .dep_pts()
            .iter()
            .map(|dep| {
                if seen_inputs.insert(*dep) {
                    self.cell(dep)
                } else {
                    CellState::Tail
                }
            }).collect();

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
                if seen_inputs.insert(*input) {
                    self.cell_at(input, parent)
                } else {
                    CellState::Tail
                }
            }).collect();

        let dep_cells = transaction
            .dep_pts()
            .iter()
            .map(|dep| {
                if seen_inputs.insert(*dep) {
                    self.cell_at(dep, parent)
                } else {
                    CellState::Tail
                }
            }).collect();

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
            if *state == CellState::Unknown {
                *state = self.cell(out_point);
            }
        }
    }
}

impl ResolvedTransaction {
    pub fn cells_iter(&self) -> Chain<slice::Iter<CellState>, slice::Iter<CellState>> {
        self.dep_cells.iter().chain(&self.input_cells)
    }

    pub fn cells_iter_mut(
        &mut self,
    ) -> Chain<slice::IterMut<CellState>, slice::IterMut<CellState>> {
        self.dep_cells.iter_mut().chain(&mut self.input_cells)
    }

    pub fn is_double_spend(&self) -> bool {
        self.cells_iter().any(|state| state.is_tail())
    }

    pub fn is_orphan(&self) -> bool {
        self.cells_iter().any(|state| state.is_unknown())
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.cells_iter().all(|state| state.is_head())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::H256;
    use std::collections::HashMap;

    struct CellMemoryDb {
        cells: HashMap<OutPoint, Option<CellOutput>>,
    }
    impl CellProvider for CellMemoryDb {
        fn cell(&self, o: &OutPoint) -> CellState {
            match self.cells.get(o) {
                Some(&Some(ref cell_output)) => CellState::Head(cell_output.clone()),
                Some(&None) => CellState::Tail,
                None => CellState::Unknown,
            }
        }

        fn cell_at(&self, o: &OutPoint, _: &H256) -> CellState {
            match self.cells.get(o) {
                Some(&Some(ref cell_output)) => CellState::Head(cell_output.clone()),
                Some(&None) => CellState::Tail,
                None => CellState::Unknown,
            }
        }
    }

    #[test]
    fn cell_provider_trait_works() {
        let mut db = CellMemoryDb {
            cells: HashMap::new(),
        };

        let p1 = OutPoint {
            hash: 0.into(),
            index: 1,
        };
        let p2 = OutPoint {
            hash: 0.into(),
            index: 2,
        };
        let p3 = OutPoint {
            hash: 0.into(),
            index: 3,
        };
        let o = CellOutput {
            capacity: 2,
            data: vec![],
            lock: H256::default(),
        };

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(CellState::Head(o), db.cell(&p1));
        assert_eq!(CellState::Tail, db.cell(&p2));
        assert_eq!(CellState::Unknown, db.cell(&p3));
    }
}
