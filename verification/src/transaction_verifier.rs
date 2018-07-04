use core::transaction::Transaction;
use error::TransactionError;
use std::collections::HashSet;

pub struct TransactionVerifier<'a> {
    pub null: NullVerifier<'a>,
    pub empty: EmptyVerifier<'a>,
    pub capacity: CapacityVerifier<'a>,
    pub duplicate_inputs: DuplicateInputsVerifier<'a>,
    pub cellbase: CellbaseVerifier<'a>,
}

impl<'a> TransactionVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        TransactionVerifier {
            null: NullVerifier::new(transaction),
            empty: EmptyVerifier::new(transaction),
            capacity: CapacityVerifier::new(transaction),
            duplicate_inputs: DuplicateInputsVerifier::new(transaction),
            cellbase: CellbaseVerifier::new(transaction),
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        self.empty.verify()?;
        self.null.verify()?;
        self.capacity.verify()?;
        self.duplicate_inputs.verify()?;
        self.cellbase.verify()?;
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
        let inputs = self.transaction.inputs.iter().collect::<HashSet<_>>();

        if inputs.len() == self.transaction.inputs.len() {
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
        if !self.transaction.is_cellbase()
            && self
                .transaction
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
            .any(|output| output.bytes_len() > (output.capacity as usize))
        {
            Err(TransactionError::OutofBound)
        } else {
            Ok(())
        }
    }
}

pub struct CellbaseVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> CellbaseVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        CellbaseVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        if !self.transaction.is_cellbase() {
            return Ok(());
        }
        if self.transaction.outputs.len() != 1 {
            Err(TransactionError::InvalidCellbase)
        } else {
            Ok(())
        }
    }
}
