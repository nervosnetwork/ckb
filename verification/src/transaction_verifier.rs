use crate::error::TransactionError;
use ckb_core::transaction::{Capacity, Transaction, TX_VERSION};
use ckb_core::{
    cell::{CellMeta, ResolvedTransaction},
    BlockNumber, Cycle,
};
use ckb_script::TransactionScriptsVerifier;
use ckb_traits::BlockMedianTimeContext;
use lru_cache::LruCache;
use occupied_capacity::OccupiedCapacity;
use std::cell::RefCell;
use std::collections::HashSet;

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
    pub fn new(
        rtx: &'a ResolvedTransaction,
        median_time_context: &'a M,
        tip_number: BlockNumber,
    ) -> Self {
        TransactionVerifier {
            version: VersionVerifier::new(&rtx.transaction),
            null: NullVerifier::new(&rtx.transaction),
            empty: EmptyVerifier::new(&rtx.transaction),
            duplicate_inputs: DuplicateInputsVerifier::new(&rtx.transaction),
            script: ScriptVerifier::new(rtx),
            capacity: CapacityVerifier::new(rtx),
            inputs: InputVerifier::new(rtx),
            valid_since: ValidSinceVerifier::new(rtx, median_time_context, tip_number),
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, TransactionError> {
        self.version.verify()?;
        self.empty.verify()?;
        self.null.verify()?;
        self.inputs.verify()?;
        self.capacity.verify()?;
        self.duplicate_inputs.verify()?;
        self.valid_since.verify()?;
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
            .fold(0, |acc, cell| acc + cell.cell_output.capacity);

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

const LOCK_TYPE_FLAG: u64 = 1 << 63;
const TIME_TYPE_FLAG: u64 = 1 << 62;
const TIMESTAMP_SCALAR: u64 = 9;
const VALUE_MUSK: u64 = 0x00ff_ffff_ffff_ffff;

/// RFC 0017
#[derive(Copy, Clone, Debug)]
struct ValidSince(u64);

impl ValidSince {
    pub fn is_absolute(self) -> bool {
        self.0 & LOCK_TYPE_FLAG == 0
    }

    #[inline]
    pub fn is_relative(self) -> bool {
        !self.is_absolute()
    }

    fn time_type_is_number(self) -> bool {
        self.0 & TIME_TYPE_FLAG == 0
    }

    #[inline]
    fn time_type_is_timestamp(self) -> bool {
        !self.time_type_is_number()
    }

    pub fn block_timestamp(self) -> Option<u64> {
        if self.time_type_is_timestamp() {
            Some(((self.0 & VALUE_MUSK) << TIMESTAMP_SCALAR) * 1000)
        } else {
            None
        }
    }

    pub fn block_number(self) -> Option<u64> {
        if self.time_type_is_number() {
            Some(self.0 & VALUE_MUSK)
        } else {
            None
        }
    }
}

/// https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md#detailed-specification
pub struct ValidSinceVerifier<'a, M> {
    rtx: &'a ResolvedTransaction,
    block_median_time_context: &'a M,
    tip_number: BlockNumber,
    median_timestamps_cache: RefCell<LruCache<BlockNumber, Option<u64>>>,
}

impl<'a, M> ValidSinceVerifier<'a, M>
where
    M: BlockMedianTimeContext,
{
    pub fn new(
        rtx: &'a ResolvedTransaction,
        block_median_time_context: &'a M,
        tip_number: BlockNumber,
    ) -> Self {
        let median_timestamps_cache = RefCell::new(LruCache::new(rtx.input_cells.len()));
        ValidSinceVerifier {
            rtx,
            block_median_time_context,
            tip_number,
            median_timestamps_cache,
        }
    }

    fn block_median_time(&self, n: BlockNumber) -> Option<u64> {
        let result = self.median_timestamps_cache.borrow().get(&n).cloned();
        match result {
            Some(r) => r,
            None => {
                let timestamp = self.block_median_time_context.block_median_time(n);
                self.median_timestamps_cache
                    .borrow_mut()
                    .insert(n, timestamp);
                timestamp
            }
        }
    }

    fn verify_absolute_lock(&self, valid_since: ValidSince) -> Result<(), TransactionError> {
        if valid_since.is_absolute() {
            if let Some(block_number) = valid_since.block_number() {
                if self.tip_number < block_number {
                    return Err(TransactionError::Immature);
                }
            }

            if let Some(block_timestamp) = valid_since.block_timestamp() {
                let tip_timestamp = self.block_median_time(self.tip_number).unwrap_or_else(|| 0);
                if tip_timestamp < block_timestamp {
                    return Err(TransactionError::Immature);
                }
            }
        }
        Ok(())
    }
    fn verify_relative_lock(
        &self,
        valid_since: ValidSince,
        cell_meta: &CellMeta,
    ) -> Result<(), TransactionError> {
        if valid_since.is_relative() {
            // cell still in tx_pool
            let cell_block_number = match cell_meta.block_number {
                Some(number) => number,
                None => return Err(TransactionError::Immature),
            };
            if let Some(block_number) = valid_since.block_number() {
                if self.tip_number < cell_block_number + block_number {
                    return Err(TransactionError::Immature);
                }
            }

            if let Some(block_timestamp) = valid_since.block_timestamp() {
                let tip_timestamp = self.block_median_time(self.tip_number).unwrap_or_else(|| 0);
                let median_timestamp = self
                    .block_median_time(cell_block_number)
                    .unwrap_or_else(|| 0);
                if tip_timestamp < median_timestamp + block_timestamp {
                    return Err(TransactionError::Immature);
                }
            }
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        for (cell_status, input) in self
            .rtx
            .input_cells
            .iter()
            .zip(self.rtx.transaction.inputs())
        {
            let cell = match cell_status.get_live() {
                Some(cell) => cell,
                None => return Err(TransactionError::Conflict),
            };
            // ignore empty valid_since
            if input.valid_since == 0 {
                continue;
            }
            let valid_since = ValidSince(input.valid_since);
            self.verify_absolute_lock(valid_since)?;
            self.verify_relative_lock(valid_since, cell)?;
        }
        Ok(())
    }
}
