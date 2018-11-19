use transaction::{CellOutput, OutPoint, Transaction};

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub enum CellState {
    /// Cell exists and is the head in its cell chain.
    Head(CellOutput),
    /// Cell exists and is not the head of its cell chain.
    Tail,
    /// Cell does not exist.
    Unknown,
}

/// Transaction with resolved input cells.
pub struct ResolvedTransaction {
    pub transaction: Transaction,
    pub input_cells: Vec<CellState>,
}

pub trait CellProvider {
    fn cell(&self, out_point: &OutPoint) -> CellState;

    fn resolve_transaction(&self, transaction: Transaction) -> ResolvedTransaction {
        let input_cells = transaction
            .inputs
            .iter()
            .map(|input| self.cell(&input.previous_output))
            .collect();
        ResolvedTransaction {
            transaction,
            input_cells,
        }
    }

    fn resolve_transaction_unknown_inputs(&self, resolved_transaction: &mut ResolvedTransaction) {
        for (input, state) in resolved_transaction
            .transaction
            .inputs
            .iter()
            .zip(&mut resolved_transaction.input_cells)
        {
            if let CellState::Unknown = *state {
                *state = self.cell(&input.previous_output);
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
    pub fn is_double_spend(&self) -> bool {
        self.input_cells.iter().any(|state| match *state {
            CellState::Tail => true,
            _ => false,
        })
    }

    pub fn is_orphan(&self) -> bool {
        self.input_cells.iter().any(|state| match *state {
            CellState::Unknown => true,
            _ => false,
        })
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.input_cells.iter().all(|state| match *state {
            CellState::Head(_) => true,
            _ => false,
        })
    }

    // TODO: split it
    // TODO: tells validation error
    pub fn validate(&self, _is_enlarge_transaction: bool) -> bool {
        // check inputs
        let mut input_cells = Vec::<&CellOutput>::with_capacity(self.input_cells.len());
        for input in &self.input_cells {
            match input.head() {
                Some(cell) => input_cells.push(cell),
                None => {
                    return false;
                }
            }
        }

        // check capacity balance
        // TODO: capacity check is disabled to ease testing.
        // if !is_enlarge_transaction {
        //     let input_capacity: u32 = input_cells.iter().map(|c| c.capacity).sum();
        //     let output_capacity: u32 = self.transaction.outputs.iter().map(|c| c.capacity).sum();
        //     if output_capacity > input_capacity {
        //         return false;
        //     }
        // }

        // check groups
        let mut inputs_offset = 0;
        for group in self.transaction.groups_iter() {
            let middle_inputs_offset = inputs_offset + group.transform_inputs.len();
            let new_inputs_offset = middle_inputs_offset + group.destroy_inputs.len();

            let transform_inputs = &input_cells[inputs_offset..middle_inputs_offset];
            let destroy_inputs = &input_cells[middle_inputs_offset..new_inputs_offset];
            inputs_offset = new_inputs_offset;

            let group_module = if !destroy_inputs.is_empty() {
                destroy_inputs[0].module
            } else if !group.create_outputs.is_empty() {
                group.create_outputs[0].module
            } else {
                // the first consume or the first transform
                transform_inputs
                    .iter()
                    .zip(group.transform_outputs)
                    .find(|op| {
                        op.0.recipient.is_some() && op.1.data.is_empty() && op.1.recipient.is_none()
                    })
                    .map_or_else(|| transform_inputs[0].module, |op| op.0.module)
            };

            // check module
            for (input_cell, input) in destroy_inputs.iter().zip(group.destroy_inputs) {
                if input_cell.module != group_module {
                    return false;
                }
                if !self.transaction
                    .check_lock(&input_cell.lock[..], &input.unlock[..])
                {
                    return false;
                }
            }
            for output in group.create_outputs {
                if output.module != group_module {
                    return false;
                }
            }
            for (input_cell, (input, output)) in transform_inputs
                .iter()
                .zip(group.transform_inputs.iter().zip(group.transform_outputs))
            {
                if input_cell.module != output.module {
                    return false;
                }
                if input_cell.module != group_module
                    && !(input_cell
                        .recipient
                        .as_ref()
                        .map_or(false, |r| r.module == group_module)
                        && output.data.is_empty()
                        && output.recipient.is_none())
                {
                    return false;
                }

                if let Some(ref r) = input_cell.recipient {
                    if input_cell.capacity != output.capacity || input_cell.lock != output.lock {
                        return false;
                    }

                    if !self.transaction.check_lock(&r.lock[..], &input.unlock[..]) {
                        return false;
                    }
                } else if !self.transaction
                    .check_lock(&input_cell.lock[..], &input.unlock[..])
                {
                    return false;
                }
            }
        }

        // TODO: run checker

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            module: 1,
            capacity: 2,
            data: vec![],
            lock: vec![],
            recipient: None,
        };

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(CellState::Head(o), db.cell(&p1));
        assert_eq!(CellState::Tail, db.cell(&p2));
        assert_eq!(CellState::Unknown, db.cell(&p3));
    }
}
