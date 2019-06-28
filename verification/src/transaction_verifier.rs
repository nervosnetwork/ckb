use crate::error::TransactionError;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::{
    cell::{CellMeta, ResolvedOutPoint, ResolvedTransaction},
    transaction::{Transaction, TX_VERSION},
    BlockNumber, Cycle, EpochNumber,
};
use ckb_logger::info_target;
use ckb_resource::CODE_HASH_DAO;
use ckb_script::{ScriptConfig, TransactionScriptsVerifier};
use ckb_script_data_loader::DataLoader;
use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainStore};
use ckb_traits::BlockMedianTimeContext;
use lru_cache::LruCache;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

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
        consensus: &'a Consensus,
    ) -> Self {
        ContextualTransactionVerifier {
            maturity: MaturityVerifier::new(&rtx, block_number, consensus.cellbase_maturity()),
            since: SinceVerifier::new(rtx, median_time_context, block_number, epoch_number),
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
    pub capacity: CapacityVerifier<'a, CS>,
    pub duplicate_deps: DuplicateDepsVerifier<'a>,
    pub script: ScriptVerifier<'a, CS>,
    pub since: SinceVerifier<'a, M>,
}

impl<'a, M, CS> TransactionVerifier<'a, M, CS>
where
    M: BlockMedianTimeContext,
    CS: ChainStore,
{
    pub fn new(
        rtx: &'a ResolvedTransaction,
        median_time_context: &'a M,
        block_number: BlockNumber,
        epoch_number: EpochNumber,
        consensus: &'a Consensus,
        script_config: &'a ScriptConfig,
        chain_store: &'a Arc<CS>,
    ) -> Self {
        TransactionVerifier {
            version: VersionVerifier::new(&rtx.transaction),
            size: SizeVerifier::new(&rtx.transaction, consensus.max_block_bytes()),
            empty: EmptyVerifier::new(&rtx.transaction),
            maturity: MaturityVerifier::new(&rtx, block_number, consensus.cellbase_maturity()),
            duplicate_deps: DuplicateDepsVerifier::new(&rtx.transaction),
            script: ScriptVerifier::new(rtx, chain_store, script_config),
            capacity: CapacityVerifier::new(rtx, chain_store),
            since: SinceVerifier::new(rtx, median_time_context, block_number, epoch_number),
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, TransactionError> {
        self.version.verify()?;
        self.size.verify()?;
        self.empty.verify()?;
        self.maturity.verify()?;
        self.capacity.verify()?;
        self.duplicate_deps.verify()?;
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
    chain_store: &'a Arc<CS>,
    resolved_transaction: &'a ResolvedTransaction<'a>,
    script_config: &'a ScriptConfig,
}

impl<'a, CS: ChainStore> ScriptVerifier<'a, CS> {
    pub fn new(
        resolved_transaction: &'a ResolvedTransaction,
        chain_store: &'a Arc<CS>,
        script_config: &'a ScriptConfig,
    ) -> Self {
        ScriptVerifier {
            chain_store,
            resolved_transaction,
            script_config,
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, TransactionError> {
        let data_loader = DataLoaderWrapper::new(Arc::clone(&self.chain_store));
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
                .filter_map(ResolvedOutPoint::cell)
                .any(cellbase_immature)
        };
        let dep_immature_spend = || {
            self.transaction
                .resolved_deps
                .iter()
                .filter_map(ResolvedOutPoint::cell)
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

pub struct CapacityVerifier<'a, CS> {
    chain_store: &'a Arc<CS>,
    resolved_transaction: &'a ResolvedTransaction<'a>,
}

impl<'a, CS: ChainStore> CapacityVerifier<'a, CS> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction, chain_store: &'a Arc<CS>) -> Self {
        CapacityVerifier {
            chain_store,
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

        for output in self.resolved_transaction.transaction.outputs().iter() {
            if output.is_lack_of_capacity()? {
                return Err(TransactionError::InsufficientCellCapacity);
            }
        }

        Ok(())
    }

    fn valid_dao_withdraw_transaction(&self) -> bool {
        let data_loader = DataLoaderWrapper::new(Arc::clone(&self.chain_store));
        self.resolved_transaction
            .resolved_inputs
            .iter()
            .any(|input| {
                input
                    .cell()
                    .map(|cell| {
                        let output = data_loader.lazy_load_cell_output(&cell);
                        output
                            .type_
                            .map(|type_| type_.code_hash == CODE_HASH_DAO)
                            .unwrap_or(false)
                    })
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
    median_timestamps_cache: RefCell<LruCache<BlockNumber, u64>>,
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
    ) -> Self {
        let median_timestamps_cache = RefCell::new(LruCache::new(rtx.resolved_inputs.len()));
        SinceVerifier {
            rtx,
            block_median_time_context,
            block_number,
            epoch_number,
            median_timestamps_cache,
        }
    }

    fn block_median_time(&self, n: BlockNumber) -> u64 {
        let result = self.median_timestamps_cache.borrow().get(&n).cloned();
        match result {
            Some(median_time) => median_time,
            None => {
                let median_time = self
                    .block_median_time_context
                    .get_block_hash(n)
                    .map(|block_hash| {
                        self.block_median_time_context
                            .block_median_time(n, &block_hash)
                    })
                    .unwrap_or(0);
                info_target!(
                    crate::LOG_TARGET,
                    "median_time {}, number {}",
                    median_time,
                    n
                );
                self.median_timestamps_cache
                    .borrow_mut()
                    .insert(n, median_time);
                median_time
            }
        }
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
                    let tip_timestamp = self.block_median_time(self.block_number.saturating_sub(1));
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
            // cell still in tx_pool
            let (cell_block_number, cell_epoch_number) = match cell_meta.block_info {
                Some(ref block_info) => (block_info.number, block_info.epoch),
                None => return Err(TransactionError::Immature),
            };
            match since.extract_metric() {
                Some(SinceMetric::BlockNumber(block_number)) => {
                    if self.block_number < cell_block_number + block_number {
                        return Err(TransactionError::Immature);
                    }
                }
                Some(SinceMetric::EpochNumber(epoch_number)) => {
                    if self.epoch_number < cell_epoch_number + epoch_number {
                        return Err(TransactionError::Immature);
                    }
                }
                Some(SinceMetric::Timestamp(timestamp)) => {
                    // pass_median_time(current_block) starts with tip block, which is the
                    // parent of current block.
                    // pass_median_time(input_cell's block) starts with cell_block_number - 1,
                    // which is the parent of input_cell's block
                    let cell_median_timestamp =
                        self.block_median_time(cell_block_number.saturating_sub(1));
                    let current_median_time =
                        self.block_median_time(self.block_number.saturating_sub(1));
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
        for (resolved_out_point, input) in self
            .rtx
            .resolved_inputs
            .iter()
            .zip(self.rtx.transaction.inputs())
        {
            if resolved_out_point.cell().is_none() {
                continue;
            }
            let cell_meta = resolved_out_point.cell().unwrap();
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
