use bigint::H256;
use std::collections::HashSet;
use std::iter::Chain;
use std::slice;
use transaction::{CellOutput, OutPoint, Transaction};

pub trait CellState: Send {
    fn tail() -> Self;
    fn unknown() -> Self;
    fn head(&self) -> Option<&CellOutput>;
    fn take_head(self) -> Option<CellOutput>;
    fn is_head(&self) -> bool;
    fn is_unknown(&self) -> bool;
    fn is_tail(&self) -> bool;
}

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction<S> {
    pub transaction: Transaction,
    pub dep_cells: Vec<S>,
    pub input_cells: Vec<S>,
}

pub trait CellProvider {
    type State: CellState;

    fn cell(&self, out_point: &OutPoint) -> Self::State;

    fn cell_at(&self, out_point: &OutPoint, parent: &H256) -> Self::State;

    fn resolve_transaction(&self, transaction: &Transaction) -> ResolvedTransaction<Self::State> {
        let mut seen_outpoints = HashSet::new();

        let input_cells = transaction
            .inputs
            .iter()
            .map(|input| {
                if seen_outpoints.insert(input.previous_output) {
                    self.cell(&input.previous_output)
                } else {
                    Self::State::tail()
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
                    Self::State::tail()
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
    ) -> ResolvedTransaction<Self::State> {
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

    fn resolve_transaction_unknown_inputs(
        &self,
        resolved_transaction: &mut ResolvedTransaction<Self::State>,
    ) {
        for (out_point, state) in resolved_transaction.transaction.out_points_iter().zip(
            resolved_transaction
                .dep_cells
                .iter_mut()
                .chain(&mut resolved_transaction.input_cells),
        ) {
            if state.is_unknown() {
                *state = self.cell(out_point);
            }
        }
    }
}

impl<S: CellState> ResolvedTransaction<S> {
    pub fn cells_iter(&self) -> Chain<slice::Iter<S>, slice::Iter<S>> {
        self.dep_cells.iter().chain(&self.input_cells)
    }

    pub fn cells_iter_mut(&mut self) -> Chain<slice::IterMut<S>, slice::IterMut<S>> {
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

    #[derive(Clone, PartialEq, Debug)]
    pub enum DummyCellState {
        Head(CellOutput),
        Tail,
        Unknown,
    }

    impl CellState for DummyCellState {
        fn tail() -> Self {
            DummyCellState::Tail
        }

        fn unknown() -> Self {
            DummyCellState::Unknown
        }

        fn head(&self) -> Option<&CellOutput> {
            match *self {
                DummyCellState::Head(ref output) => Some(output),
                _ => None,
            }
        }

        fn take_head(self) -> Option<CellOutput> {
            match self {
                DummyCellState::Head(output) => Some(output),
                _ => None,
            }
        }

        fn is_head(&self) -> bool {
            match *self {
                DummyCellState::Head(_) => true,
                _ => false,
            }
        }
        fn is_unknown(&self) -> bool {
            match *self {
                DummyCellState::Unknown => true,
                _ => false,
            }
        }
        fn is_tail(&self) -> bool {
            match *self {
                DummyCellState::Tail => true,
                _ => false,
            }
        }
    }

    struct CellMemoryDb {
        cells: HashMap<OutPoint, Option<CellOutput>>,
    }
    impl CellProvider for CellMemoryDb {
        type State = DummyCellState;

        fn cell(&self, out_point: &OutPoint) -> Self::State {
            match self.cells.get(out_point) {
                Some(&Some(ref cell_output)) => DummyCellState::Head(cell_output.clone()),
                Some(&None) => DummyCellState::Tail,
                None => DummyCellState::Unknown,
            }
        }

        fn cell_at(&self, out_point: &OutPoint, _: &H256) -> Self::State {
            match self.cells.get(out_point) {
                Some(&Some(ref cell_output)) => DummyCellState::Head(cell_output.clone()),
                Some(&None) => DummyCellState::Tail,
                None => DummyCellState::Unknown,
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

        assert_eq!(DummyCellState::Head(o), db.cell(&p1));
        assert_eq!(DummyCellState::Tail, db.cell(&p2));
        assert_eq!(DummyCellState::Unknown, db.cell(&p3));
    }
}
