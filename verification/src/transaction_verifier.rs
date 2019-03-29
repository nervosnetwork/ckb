use crate::error::TransactionError;
use ckb_core::transaction::{Capacity, Transaction, TX_VERSION};
use ckb_core::{cell::ResolvedTransaction, BlockNumber, Cycle};
use ckb_script::TransactionScriptsVerifier;
use ckb_traits::BlockMedianTimeContext;
use numext_fixed_hash::H256;
use occupied_capacity::OccupiedCapacity;
use std::collections::HashSet;

pub struct BlockContext<M> {
    pub block_median_time_context: M,
    pub tip_number: BlockNumber,
    pub tip_hash: H256,
}

pub struct TransactionVerifier<'a, M> {
    pub version: VersionVerifier<'a>,
    pub null: NullVerifier<'a>,
    pub empty: EmptyVerifier<'a>,
    pub capacity: CapacityVerifier<'a>,
    pub duplicate_inputs: DuplicateInputsVerifier<'a>,
    pub inputs: InputVerifier<'a>,
    pub script: ScriptVerifier<'a>,
    pub valid_since: ValidSinceVerifier<'a, M>,
}

impl<'a, M> TransactionVerifier<'a, M>
where
    M: BlockMedianTimeContext,
{
    pub fn new(rtx: &'a ResolvedTransaction, block_context: &'a BlockContext<M>) -> Self {
        TransactionVerifier {
            version: VersionVerifier::new(&rtx.transaction),
            null: NullVerifier::new(&rtx.transaction),
            empty: EmptyVerifier::new(&rtx.transaction),
            duplicate_inputs: DuplicateInputsVerifier::new(&rtx.transaction),
            script: ScriptVerifier::new(rtx),
            capacity: CapacityVerifier::new(rtx),
            inputs: InputVerifier::new(rtx),
            valid_since: ValidSinceVerifier::new(&rtx.transaction, &block_context),
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, TransactionError> {
        self.version.verify()?;
        self.empty.verify()?;
        self.null.verify()?;
        self.inputs.verify()?;
        self.capacity.verify()?;
        self.duplicate_inputs.verify()?;
        let cycles = self.script.verify(max_cycles)?;
        Ok(cycles)
    }
}

pub struct VersionVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> VersionVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        VersionVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        if self.transaction.version() != TX_VERSION {
            return Err(TransactionError::Version);
        }
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
        for cs in &self.resolved_transaction.input_cells {
            if cs.is_dead() {
                return Err(TransactionError::Conflict);
            } else if cs.is_unknown() {
                return Err(TransactionError::UnknownInput);
            }
        }

        for cs in &self.resolved_transaction.dep_cells {
            if cs.is_dead() {
                return Err(TransactionError::Conflict);
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

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, TransactionError> {
        TransactionScriptsVerifier::new(&self.resolved_transaction)
            .verify(max_cycles)
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
        let mut seen = HashSet::with_capacity(self.transaction.inputs().len());

        if transaction.inputs().iter().all(|id| seen.insert(id)) {
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
            .filter_map(|state| state.get_live())
            .fold(0, |acc, output| acc + output.capacity);

        let outputs_total = self
            .resolved_transaction
            .transaction
            .outputs()
            .iter()
            .fold(0, |acc, output| acc + output.capacity);

        if inputs_total < outputs_total {
            return Err(TransactionError::OutputsSumOverflow);
        }
        if self
            .resolved_transaction
            .transaction
            .outputs()
            .iter()
            .any(|output| output.occupied_capacity() as Capacity > output.capacity)
        {
            return Err(TransactionError::CapacityOverflow);
        }
        Ok(())
    }
}

pub struct ValidSinceVerifier<'a, M> {
    transaction: &'a Transaction,
    block_context: &'a BlockContext<M>,
}

impl<'a, M> ValidSinceVerifier<'a, M>
where
    M: BlockMedianTimeContext,
{
    pub fn new(transaction: &'a Transaction, block_context: &'a BlockContext<M>) -> Self {
        ValidSinceVerifier {
            transaction,
            block_context,
        }
    }
    // https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md#detailed-specification
    pub fn verify(&self) -> Result<(), TransactionError> {
        let valid_since = self.transaction.valid_since();
        if valid_since == 0 {
            return Ok(());
        }
        if valid_since >> 63 == 0 && self.block_context.tip_number < valid_since {
            return Err(TransactionError::Immature);
        }
        if self
            .block_context
            .block_median_time_context
            .block_median_time(&self.block_context.tip_hash)
            .unwrap_or_else(|| 0)
            < (valid_since ^ (1 << 63)) * 512 * 1000
        {
            return Err(TransactionError::Immature);
        }
        Ok(())
    }
}
