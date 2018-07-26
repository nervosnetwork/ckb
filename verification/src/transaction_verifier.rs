use core::cell::{CellState, ResolvedTransaction};
use core::transaction::{Capacity, Transaction};
use error::TransactionError;
use std::collections::HashSet;

pub struct TransactionVerifier<'a> {
    pub null: NullVerifier<'a>,
    pub empty: EmptyVerifier<'a>,
    pub capacity: CapacityVerifier<'a>,
    pub duplicate_inputs: DuplicateInputsVerifier<'a>,
    pub inputs: InputVerifier<'a>,
}

impl<'a> TransactionVerifier<'a> {
    pub fn new(rtx: ResolvedTransaction<'a>) -> TransactionVerifier<'a> {
        TransactionVerifier {
            null: NullVerifier::new(rtx.transaction),
            empty: EmptyVerifier::new(rtx.transaction),
            capacity: CapacityVerifier::new(rtx.transaction),
            duplicate_inputs: DuplicateInputsVerifier::new(rtx.transaction),
            inputs: InputVerifier::new(rtx),
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        self.empty.verify()?;
        self.null.verify()?;
        self.capacity.verify()?;
        self.duplicate_inputs.verify()?;
        self.inputs.verify()?;
        Ok(())
    }
}

pub struct InputVerifier<'a> {
    resolved_transaction: ResolvedTransaction<'a>,
}

impl<'a> InputVerifier<'a> {
    pub fn new(resolved_transaction: ResolvedTransaction<'a>) -> InputVerifier {
        InputVerifier {
            resolved_transaction,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let mut inputs = self.resolved_transaction.transaction.inputs.iter();
        for cs in &self.resolved_transaction.input_cells {
            match *cs {
                CellState::Head(ref input)
                | CellState::Pool(ref input)
                | CellState::Orphan(ref input) => {
                    if input.lock != inputs.next().unwrap().unlock.redeem_script_hash() {
                        return Err(TransactionError::InvalidScript);
                    }
                }
                CellState::Tail => return Err(TransactionError::DoubleSpent),
                CellState::Unknown => return Err(TransactionError::UnknownInput),
            }
        }

        for cs in &self.resolved_transaction.dep_cells {
            match *cs {
                CellState::Tail => return Err(TransactionError::DoubleSpent),
                CellState::Unknown => return Err(TransactionError::UnknownInput),
                _ => {}
            }
        }
        Ok(())
    }
}

pub struct EmptyVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> EmptyVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        EmptyVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        if self.transaction.is_empty() {
            Err(TransactionError::Empty)
        } else {
            Ok(())
        }
    }
}

pub struct DuplicateInputsVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> DuplicateInputsVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        DuplicateInputsVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let transaction = self.transaction;
        let inputs = transaction.inputs.iter().collect::<HashSet<_>>();

        if inputs.len() == transaction.inputs.len() {
            Ok(())
        } else {
            Err(TransactionError::DuplicateInputs)
        }
    }
}

pub struct NullVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> NullVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        NullVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let transaction = self.transaction;
        if !transaction.is_cellbase()
            && transaction
                .inputs
                .iter()
                .any(|input| input.previous_output.is_null())
        {
            Err(TransactionError::NullNonCellbase)
        } else {
            Ok(())
        }
    }
}

pub struct CapacityVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> CapacityVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        CapacityVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        if self
            .transaction
            .outputs
            .iter()
            .any(|output| output.bytes_len() as Capacity > output.capacity)
        {
            Err(TransactionError::OutofBound)
        } else {
            Ok(())
        }
    }
}
