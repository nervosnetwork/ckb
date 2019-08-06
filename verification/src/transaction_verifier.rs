use crate::error::TransactionError;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::{
    cell::{CellMeta, ResolvedDep, ResolvedInput, ResolvedTransaction},
    transaction::{CellOutput, Transaction, TX_VERSION},
    BlockNumber, Capacity, Cycle, EpochNumber,
};
use ckb_resource::CODE_HASH_DAO;
use ckb_script::{ScriptConfig, TransactionScriptsVerifier};
use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainStore};
use ckb_traits::BlockMedianTimeContext;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::cell::RefCell;
use std::collections::HashSet;

pub struct ContextualTransactionVerifier<'a, M> {
    pub maturity: MaturityVerifier<'a>,
    pub since: SinceVerifier<'a, M>,
}

impl<'a, M> ContextualTransactionVerifier<'a, M>
where
    M: BlockMedianTimeContext,
{
    pub fn new(
        rtx: &'a ResolvedTransaction,
        median_time_context: &'a M,
        block_number: BlockNumber,
        epoch_number: EpochNumber,
        parent_hash: &'a H256,
        consensus: &'a Consensus,
    ) -> Self {
        ContextualTransactionVerifier {
            maturity: MaturityVerifier::new(&rtx, block_number, consensus.cellbase_maturity()),
            since: SinceVerifier::new(
                rtx,
                median_time_context,
                block_number,
                epoch_number,
                parent_hash,
            ),
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        self.maturity.verify()?;
        self.since.verify()?;
        Ok(())
    }
}

pub struct TransactionVerifier<'a, M, CS> {
    pub version: VersionVerifier<'a>,
    pub size: SizeVerifier<'a>,
    pub empty: EmptyVerifier<'a>,
    pub maturity: MaturityVerifier<'a>,
    pub capacity: CapacityVerifier<'a>,
    pub duplicate_deps: DuplicateDepsVerifier<'a>,
    pub outputs_data_verifier: OutputsDataVerifier<'a>,
    pub script: ScriptVerifier<'a, CS>,
    pub since: SinceVerifier<'a, M>,
}

impl<'a, M, CS> TransactionVerifier<'a, M, CS>
where
    M: BlockMedianTimeContext,
    CS: ChainStore<'a>,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rtx: &'a ResolvedTransaction,
        median_time_context: &'a M,
        block_number: BlockNumber,
        epoch_number: EpochNumber,
        parent_hash: &'a H256,
        consensus: &'a Consensus,
        script_config: &'a ScriptConfig,
        chain_store: &'a CS,
    ) -> Self {
        TransactionVerifier {
            version: VersionVerifier::new(&rtx.transaction),
            size: SizeVerifier::new(&rtx.transaction, consensus.max_block_bytes()),
            empty: EmptyVerifier::new(&rtx.transaction),
            maturity: MaturityVerifier::new(&rtx, block_number, consensus.cellbase_maturity()),
            duplicate_deps: DuplicateDepsVerifier::new(&rtx.transaction),
            outputs_data_verifier: OutputsDataVerifier::new(&rtx.transaction),
            script: ScriptVerifier::new(rtx, chain_store, script_config),
            capacity: CapacityVerifier::new(rtx),
            since: SinceVerifier::new(
                rtx,
                median_time_context,
                block_number,
                epoch_number,
                parent_hash,
            ),
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, TransactionError> {
        self.version.verify()?;
        self.size.verify()?;
        self.empty.verify()?;
        self.maturity.verify()?;
        self.capacity.verify()?;
        self.duplicate_deps.verify()?;
        self.outputs_data_verifier.verify()?;
        self.since.verify()?;
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

pub struct SizeVerifier<'a> {
    transaction: &'a Transaction,
    block_bytes_limit: u64,
}

impl<'a> SizeVerifier<'a> {
    pub fn new(transaction: &'a Transaction, block_bytes_limit: u64) -> Self {
        SizeVerifier {
            transaction,
            block_bytes_limit,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let size = self.transaction.serialized_size() as u64;
        if size <= self.block_bytes_limit {
            Ok(())
        } else {
            Err(TransactionError::ExceededMaximumBlockBytes)
        }
    }
}

pub struct ScriptVerifier<'a, CS> {
    chain_store: &'a CS,
    resolved_transaction: &'a ResolvedTransaction<'a>,
    script_config: &'a ScriptConfig,
}

impl<'a, CS: ChainStore<'a>> ScriptVerifier<'a, CS> {
    pub fn new(
        resolved_transaction: &'a ResolvedTransaction,
        chain_store: &'a CS,
        script_config: &'a ScriptConfig,
    ) -> Self {
        ScriptVerifier {
            chain_store,
            resolved_transaction,
            script_config,
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, TransactionError> {
        let data_loader = DataLoaderWrapper::new(self.chain_store);
        TransactionScriptsVerifier::new(
            &self.resolved_transaction,
            &data_loader,
            &self.script_config,
        )
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

pub struct MaturityVerifier<'a> {
    transaction: &'a ResolvedTransaction<'a>,
    block_number: BlockNumber,
    cellbase_maturity: BlockNumber,
}

impl<'a> MaturityVerifier<'a> {
    pub fn new(
        transaction: &'a ResolvedTransaction,
        block_number: BlockNumber,
        cellbase_maturity: BlockNumber,
    ) -> Self {
        MaturityVerifier {
            transaction,
            block_number,
            cellbase_maturity,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let cellbase_immature = |meta: &CellMeta| -> bool {
            meta.is_cellbase()
                && self.block_number
                    < meta
                        .block_info
                        .as_ref()
                        .expect("cell meta should have block number when transaction verify")
                        .number
                        + self.cellbase_maturity
        };

        let input_immature_spend = || {
            self.transaction
                .resolved_inputs
                .iter()
                .map(ResolvedInput::cell)
                .any(cellbase_immature)
        };
        let dep_immature_spend = || {
            self.transaction
                .resolved_deps
                .iter()
                .filter_map(ResolvedDep::cell)
                .any(cellbase_immature)
        };

        if input_immature_spend() || dep_immature_spend() {
            Err(TransactionError::CellbaseImmaturity)
        } else {
            Ok(())
        }
    }
}

pub struct DuplicateDepsVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> DuplicateDepsVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        DuplicateDepsVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let transaction = self.transaction;
        let mut seen = HashSet::with_capacity(self.transaction.deps().len());

        if transaction.deps().iter().all(|id| seen.insert(id)) {
            Ok(())
        } else {
            Err(TransactionError::DuplicateDeps)
        }
    }
}

pub struct CapacityVerifier<'a> {
    resolved_transaction: &'a ResolvedTransaction<'a>,
}

impl<'a> CapacityVerifier<'a> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction) -> Self {
        CapacityVerifier {
            resolved_transaction,
        }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        // skip OutputsSumOverflow verification for resolved cellbase and DAO
        // withdraw transactions.
        // cellbase's outputs are verified by RewardVerifier
        // DAO withdraw transaction is verified via the type script of DAO cells
        if !(self.resolved_transaction.is_cellbase() || self.valid_dao_withdraw_transaction()) {
            let inputs_total = self.resolved_transaction.inputs_capacity()?;
            let outputs_total = self.resolved_transaction.outputs_capacity()?;

            if inputs_total < outputs_total {
                return Err(TransactionError::OutputsSumOverflow);
            }
        }

        for (output, data) in self
            .resolved_transaction
            .transaction
            .outputs_with_data_iter()
        {
            if output.is_lack_of_capacity(Capacity::bytes(data.len())?)? {
                return Err(TransactionError::InsufficientCellCapacity);
            }
        }

        Ok(())
    }

    fn valid_dao_withdraw_transaction(&self) -> bool {
        self.resolved_transaction
            .resolved_inputs
            .iter()
            .any(|input| {
                input
                    .cell()
                    .cell_output
                    .type_
                    .as_ref()
                    .map(|type_| type_.code_hash == CODE_HASH_DAO)
                    .unwrap_or(false)
            })
    }
}

const LOCK_TYPE_FLAG: u64 = 1 << 63;
const METRIC_TYPE_FLAG_MASK: u64 = 0x6000_0000_0000_0000;
const VALUE_MASK: u64 = 0x00ff_ffff_ffff_ffff;
const REMAIN_FLAGS_BITS: u64 = 0x1f00_0000_0000_0000;

enum SinceMetric {
    BlockNumber(u64),
    EpochNumber(u64),
    Timestamp(u64),
}

/// RFC 0017
#[derive(Copy, Clone, Debug)]
pub(crate) struct Since(pub(crate) u64);

impl Since {
    pub fn is_absolute(self) -> bool {
        self.0 & LOCK_TYPE_FLAG == 0
    }

    #[inline]
    pub fn is_relative(self) -> bool {
        !self.is_absolute()
    }

    pub fn flags_is_valid(self) -> bool {
        (self.0 & REMAIN_FLAGS_BITS == 0)
            && ((self.0 & METRIC_TYPE_FLAG_MASK) != METRIC_TYPE_FLAG_MASK)
    }

    fn extract_metric(self) -> Option<SinceMetric> {
        let value = self.0 & VALUE_MASK;
        match self.0 & METRIC_TYPE_FLAG_MASK {
            //0b0000_0000
            0x0000_0000_0000_0000 => Some(SinceMetric::BlockNumber(value)),
            //0b0010_0000
            0x2000_0000_0000_0000 => Some(SinceMetric::EpochNumber(value)),
            //0b0100_0000
            0x4000_0000_0000_0000 => Some(SinceMetric::Timestamp(value * 1000)),
            _ => None,
        }
    }
}

/// https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md#detailed-specification
pub struct SinceVerifier<'a, M> {
    rtx: &'a ResolvedTransaction<'a>,
    block_median_time_context: &'a M,
    block_number: BlockNumber,
    epoch_number: EpochNumber,
    parent_hash: &'a H256,
    median_timestamps_cache: RefCell<LruCache<H256, u64>>,
}

impl<'a, M> SinceVerifier<'a, M>
where
    M: BlockMedianTimeContext,
{
    pub fn new(
        rtx: &'a ResolvedTransaction,
        block_median_time_context: &'a M,
        block_number: BlockNumber,
        epoch_number: BlockNumber,
        parent_hash: &'a H256,
    ) -> Self {
        let median_timestamps_cache = RefCell::new(LruCache::new(rtx.resolved_inputs.len()));
        SinceVerifier {
            rtx,
            block_median_time_context,
            block_number,
            epoch_number,
            parent_hash,
            median_timestamps_cache,
        }
    }

    fn parent_median_time(&self, block_hash: &H256) -> u64 {
        let (_, _, parent_hash) = self
            .block_median_time_context
            .timestamp_and_parent(block_hash);
        self.block_median_time(&parent_hash)
    }

    fn block_median_time(&self, block_hash: &H256) -> u64 {
        if let Some(median_time) = self.median_timestamps_cache.borrow().get(block_hash) {
            return *median_time;
        }

        let median_time = self.block_median_time_context.block_median_time(block_hash);
        self.median_timestamps_cache
            .borrow_mut()
            .insert(block_hash.clone(), median_time);
        median_time
    }

    fn verify_absolute_lock(&self, since: Since) -> Result<(), TransactionError> {
        if since.is_absolute() {
            match since.extract_metric() {
                Some(SinceMetric::BlockNumber(block_number)) => {
                    if self.block_number < block_number {
                        return Err(TransactionError::Immature);
                    }
                }
                Some(SinceMetric::EpochNumber(epoch_number)) => {
                    if self.epoch_number < epoch_number {
                        return Err(TransactionError::Immature);
                    }
                }
                Some(SinceMetric::Timestamp(timestamp)) => {
                    let tip_timestamp = self.block_median_time(self.parent_hash);
                    if tip_timestamp < timestamp {
                        return Err(TransactionError::Immature);
                    }
                }
                None => {
                    return Err(TransactionError::InvalidSince);
                }
            }
        }
        Ok(())
    }

    fn verify_relative_lock(
        &self,
        since: Since,
        cell_meta: &CellMeta,
    ) -> Result<(), TransactionError> {
        if since.is_relative() {
            let cell = match cell_meta.block_info {
                Some(ref block_info) => block_info,
                None => return Err(TransactionError::Immature),
            };
            match since.extract_metric() {
                Some(SinceMetric::BlockNumber(block_number)) => {
                    if self.block_number < cell.number + block_number {
                        return Err(TransactionError::Immature);
                    }
                }
                Some(SinceMetric::EpochNumber(epoch_number)) => {
                    if self.epoch_number < cell.epoch + epoch_number {
                        return Err(TransactionError::Immature);
                    }
                }
                Some(SinceMetric::Timestamp(timestamp)) => {
                    // pass_median_time(current_block) starts with tip block, which is the
                    // parent of current block.
                    // pass_median_time(input_cell's block) starts with cell_block_number - 1,
                    // which is the parent of input_cell's block
                    let cell_median_timestamp = self.parent_median_time(&cell.hash);
                    let current_median_time = self.block_median_time(self.parent_hash);
                    if current_median_time < cell_median_timestamp + timestamp {
                        return Err(TransactionError::Immature);
                    }
                }
                None => {
                    return Err(TransactionError::InvalidSince);
                }
            }
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        for (resolved_input, input) in self
            .rtx
            .resolved_inputs
            .iter()
            .zip(self.rtx.transaction.inputs())
        {
            if resolved_input.cell().out_point.is_null() {
                continue;
            }
            let cell_meta = resolved_input.cell();
            // ignore empty since
            if input.since == 0 {
                continue;
            }
            let since = Since(input.since);
            // check remain flags
            if !since.flags_is_valid() {
                return Err(TransactionError::InvalidSince);
            }

            // verify time lock
            self.verify_absolute_lock(since)?;
            self.verify_relative_lock(since, cell_meta)?;
        }
        Ok(())
    }
}

pub struct OutputsDataVerifier<'a> {
    transaction: &'a Transaction,
}

impl<'a> OutputsDataVerifier<'a> {
    pub fn new(transaction: &'a Transaction) -> Self {
        Self { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        let transaction = self.transaction;
        if transaction.outputs().len() != transaction.outputs_data().len() {
            return Err(TransactionError::OutputsDataLengthMismatch);
        }
        let valid = transaction
            .outputs_with_data_iter()
            .all(|(output, data)| CellOutput::calculate_data_hash(data) == output.data_hash);
        if !valid {
            return Err(TransactionError::OutputDataHashMismatch);
        }
        Ok(())
    }
}
