use core::cell::{CellState, ResolvedTransaction};
use core::transaction::{Capacity, Transaction};
use error::TransactionError;
use script::{SignatureVerifier, TransactionInputSigner, TransactionSignatureVerifier};
use std::collections::HashSet;

pub struct TransactionVerifier<'a, S: 'a> {
    pub null: NullVerifier<'a>,
    pub empty: EmptyVerifier<'a>,
    pub capacity: CapacityVerifier<'a, S>,
    pub duplicate_inputs: DuplicateInputsVerifier<'a>,
    pub inputs: InputVerifier<'a, S>,
    pub script: ScriptVerifier<'a>,
}

impl<'a, S: CellState> TransactionVerifier<'a, S> {
    pub fn new(rtx: &'a ResolvedTransaction<S>) -> Self {
        TransactionVerifier {
            null: NullVerifier::new(&rtx.transaction),
            empty: EmptyVerifier::new(&rtx.transaction),
            duplicate_inputs: DuplicateInputsVerifier::new(&rtx.transaction),
            script: ScriptVerifier::new(&rtx.transaction),
            capacity: CapacityVerifier::new(rtx),
            inputs: InputVerifier::new(rtx),
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        self.empty.verify()?;
        self.null.verify()?;
        self.capacity.verify()?;
        self.duplicate_inputs.verify()?;
        self.inputs.verify()?;
        self.script.verify()?;
        Ok(())
    }
}

pub struct InputVerifier<'a, S: 'a> {
    resolved_transaction: &'a ResolvedTransaction<S>,
}

impl<'a, S: CellState> InputVerifier<'a, S> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction<S>) -> Self {
        InputVerifier {
            resolved_transaction,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let mut inputs = self.resolved_transaction.transaction.inputs.iter();
        for cs in &self.resolved_transaction.input_cells {
            if cs.is_head() {
                if let Some(ref input) = cs.head() {
                    if input.lock != inputs.next().unwrap().unlock.redeem_script_hash() {
                        return Err(TransactionError::InvalidScript);
                    }
                }
            } else if cs.is_tail() {
                return Err(TransactionError::DoubleSpent);
            } else if cs.is_unknown() {
                return Err(TransactionError::UnknownInput);
            }
        }

        for cs in &self.resolved_transaction.dep_cells {
            if cs.is_tail() {
                return Err(TransactionError::DoubleSpent);
            } else if cs.is_unknown() {
                return Err(TransactionError::UnknownInput);
            }
        }
        Ok(())
    }
}

pub struct ScriptVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> ScriptVerifier<'a> {
    // TODO this verifier should be replaced by VM
    pub fn new(transaction: &'a Transaction) -> Self {
        ScriptVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let signer: TransactionInputSigner = self.transaction.clone().into();

        let mut verifier = TransactionSignatureVerifier {
            signer,
            input_index: 0,
        };

        for (index, input) in self.transaction.inputs.iter().enumerate() {
            if !input.unlock.arguments.is_empty() {
                let signature = input.unlock.arguments[0].clone().into();
                verifier.input_index = index;
                if !verifier.verify(&signature) {
                    return Err(TransactionError::InvalidSignature);
                }
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
        if transaction
            .inputs
            .iter()
            .any(|input| input.previous_output.is_null())
        {
            Err(TransactionError::NullInput)
        } else {
            Ok(())
        }
    }
}

pub struct CapacityVerifier<'a, S: 'a> {
    resolved_transaction: &'a ResolvedTransaction<S>,
}

impl<'a, S: CellState> CapacityVerifier<'a, S> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction<S>) -> Self {
        CapacityVerifier {
            resolved_transaction,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let inputs_total = self
            .resolved_transaction
            .input_cells
            .iter()
            .filter_map(|state| state.head())
            .fold(0, |acc, output| acc + output.capacity);

        let outputs_total = self
            .resolved_transaction
            .transaction
            .outputs
            .iter()
            .fold(0, |acc, output| acc + output.capacity);

        if inputs_total < outputs_total {
            Err(TransactionError::InvalidCapacity)
        } else if self
            .resolved_transaction
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
