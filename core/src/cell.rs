use bigint::H256;
use std::collections::HashSet;
use std::iter::Chain;
use std::slice;
use transaction::{CellOutput, OutPoint, Transaction};

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub enum CellState {
    /// Cell exists and is the head in its cell chain.
    Head(CellOutput),
    /// Cell in Pool
    Pool(CellOutput),
    /// Cell in Orphan Pool
    Orphan(CellOutput),
    /// Cell exists and is not the head of its cell chain.
    Tail,
    /// Cell does not exist.
    Unknown,
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
        let mut seen_outpoints = HashSet::new();

        let input_cells = transaction
            .inputs
            .iter()
            .map(|input| {
                if seen_outpoints.insert(input.previous_output.clone()) {
                    self.cell(&input.previous_output)
                } else {
                    CellState::Tail
                }
            })
            .collect();
        let dep_cells = transaction
            .deps
            .iter()
            .map(|dep| {
                if seen_outpoints.insert(dep.clone()) {
                    self.cell(dep)
                } else {
                    CellState::Tail
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
        let input_cells = transaction
            .inputs
            .iter()
            .map(|input| self.cell_at(&input.previous_output, parent))
            .collect();
        let dep_cells = transaction
            .deps
            .iter()
            .map(|dep| self.cell_at(dep, parent))
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
            if let CellState::Unknown = *state {
                *state = self.cell(out_point);
            }
        }
    }
}

impl CellState {
    pub fn head(&self) -> Option<&CellOutput> {
        match *self {
            CellState::Head(ref output) => Some(output),
            _ => None,
        }
    }

    pub fn is_head(&self) -> bool {
        match *self {
            CellState::Head(_) => true,
            _ => false,
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
        self.cells_iter().any(|state| match *state {
            CellState::Tail => true,
            _ => false,
        })
    }

    pub fn is_orphan(&self) -> bool {
        self.cells_iter().any(|state| match *state {
            CellState::Unknown => true,
            _ => false,
        })
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.cells_iter().all(|state| match *state {
            CellState::Head(_) => true,
            _ => false,
        })
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
        fn cell(&self, out_point: &OutPoint) -> CellState {
            match self.cells.get(out_point) {
                Some(&Some(ref cell_output)) => CellState::Head(cell_output.clone()),
                Some(&None) => CellState::Tail,
                None => CellState::Unknown,
            }
        }

        fn cell_at(&self, out_point: &OutPoint, _: &H256) -> CellState {
            match self.cells.get(out_point) {
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
