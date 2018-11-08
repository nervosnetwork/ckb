use core::cell::ResolvedTransaction;
use core::transaction::{Capacity, Transaction};
use error::TransactionError;
use script::TransactionScriptsVerifier;
use std::collections::HashSet;

pub struct TransactionVerifier<'a> {
    pub null: NullVerifier<'a>,
    pub empty: EmptyVerifier<'a>,
    pub capacity: CapacityVerifier<'a>,
    pub duplicate_inputs: DuplicateInputsVerifier<'a>,
    pub inputs: InputVerifier<'a>,
    pub script: ScriptVerifier<'a>,
}

impl<'a> TransactionVerifier<'a> {
    pub fn new(rtx: &'a ResolvedTransaction) -> Self {
        TransactionVerifier {
            null: NullVerifier::new(&rtx.transaction),
            empty: EmptyVerifier::new(&rtx.transaction),
            duplicate_inputs: DuplicateInputsVerifier::new(&rtx.transaction),
            script: ScriptVerifier::new(rtx),
            capacity: CapacityVerifier::new(rtx),
            inputs: InputVerifier::new(rtx),
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        self.empty.verify()?;
        self.null.verify()?;
        self.capacity.verify()?;
        self.duplicate_inputs.verify()?;
        // InputVerifier should be executed before ScriptVerifier
        self.inputs.verify()?;
        self.script.verify()?;
        Ok(())
    }
}

pub struct InputVerifier<'a> {
    resolved_transaction: &'a ResolvedTransaction,
}

impl<'a> InputVerifier<'a> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction) -> Self {
        InputVerifier {
            resolved_transaction,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let mut inputs = self.resolved_transaction.transaction.inputs().iter();
        for cs in &self.resolved_transaction.input_cells {
            if cs.is_current() {
                if let Some(ref input) = cs.get_current() {
                    // TODO: remove this once VM mmap is in place so we can
                    // do P2SH within the VM.
                    if input.lock != inputs.next().unwrap().unlock.type_hash() {
                        return Err(TransactionError::InvalidScript);
                    }
                }
            } else if cs.is_old() {
                return Err(TransactionError::DoubleSpent);
            } else if cs.is_unknown() {
                return Err(TransactionError::UnknownInput);
            }
        }

        for cs in &self.resolved_transaction.dep_cells {
            if cs.is_old() {
                return Err(TransactionError::DoubleSpent);
            } else if cs.is_unknown() {
                return Err(TransactionError::UnknownInput);
            }
        }
        Ok(())
    }
}

pub struct ScriptVerifier<'a> {
    resolved_transaction: &'a ResolvedTransaction,
}

impl<'a> ScriptVerifier<'a> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction) -> Self {
        ScriptVerifier {
            resolved_transaction,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        TransactionScriptsVerifier::new(&self.resolved_transaction)
            .verify()
            .map_err(TransactionError::ScriptFailure)
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
        let inputs = transaction.inputs().iter().collect::<HashSet<_>>();

        if inputs.len() == transaction.inputs().len() {
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
        if transaction
            .inputs()
            .iter()
            .any(|input| input.previous_output.is_null())
        {
            Err(TransactionError::NullInput)
        } else {
            Ok(())
        }
    }
}

pub struct CapacityVerifier<'a> {
    resolved_transaction: &'a ResolvedTransaction,
}

impl<'a> CapacityVerifier<'a> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction) -> Self {
        CapacityVerifier {
            resolved_transaction,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let inputs_total = self
            .resolved_transaction
            .input_cells
            .iter()
            .filter_map(|state| state.get_current())
            .fold(0, |acc, output| acc + output.capacity);

        let outputs_total = self
            .resolved_transaction
            .transaction
            .outputs()
            .iter()
            .fold(0, |acc, output| acc + output.capacity);

        if inputs_total < outputs_total {
            Err(TransactionError::InvalidCapacity)
        } else if self
            .resolved_transaction
            .transaction
            .outputs()
            .iter()
            .any(|output| output.bytes_len() as Capacity > output.capacity)
        {
            Err(TransactionError::OutofBound)
        } else {
            Ok(())
        }
    }
}
