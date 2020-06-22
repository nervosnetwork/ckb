use crate::cache::CacheEntry;
use crate::error::TransactionErrorSource;
use crate::TransactionError;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_error::Error;
use ckb_script::TransactionScriptsVerifier;
use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainStore};
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    core::{
        cell::{CellMeta, ResolvedTransaction},
        BlockNumber, Capacity, Cycle, EpochNumberWithFraction, ScriptHashType, TransactionView,
        Version,
    },
    packed::Byte32,
    prelude::*,
};
use lru_cache::LruCache;
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
        epoch_number_with_fraction: EpochNumberWithFraction,
        parent_hash: Byte32,
        consensus: &'a Consensus,
    ) -> Self {
        ContextualTransactionVerifier {
            maturity: MaturityVerifier::new(
                &rtx,
                epoch_number_with_fraction,
                consensus.cellbase_maturity(),
            ),
            since: SinceVerifier::new(
                rtx,
                median_time_context,
                block_number,
                epoch_number_with_fraction,
                parent_hash,
            ),
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
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
    pub fee_calculator: FeeCalculator<'a, CS>,
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
        epoch_number_with_fraction: EpochNumberWithFraction,
        parent_hash: Byte32,
        consensus: &'a Consensus,
        chain_store: &'a CS,
    ) -> Self {
        TransactionVerifier {
            version: VersionVerifier::new(&rtx.transaction, consensus.tx_version()),
            size: SizeVerifier::new(&rtx.transaction, consensus.max_block_bytes()),
            empty: EmptyVerifier::new(&rtx.transaction),
            maturity: MaturityVerifier::new(
                &rtx,
                epoch_number_with_fraction,
                consensus.cellbase_maturity(),
            ),
            duplicate_deps: DuplicateDepsVerifier::new(&rtx.transaction),
            outputs_data_verifier: OutputsDataVerifier::new(&rtx.transaction),
            script: ScriptVerifier::new(rtx, chain_store),
            capacity: CapacityVerifier::new(rtx, consensus.dao_type_hash()),
            since: SinceVerifier::new(
                rtx,
                median_time_context,
                block_number,
                epoch_number_with_fraction,
                parent_hash,
            ),
            fee_calculator: FeeCalculator::new(rtx, &consensus, &chain_store),
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<CacheEntry, Error> {
        self.version.verify()?;
        self.size.verify()?;
        self.empty.verify()?;
        self.maturity.verify()?;
        self.capacity.verify()?;
        self.duplicate_deps.verify()?;
        self.outputs_data_verifier.verify()?;
        self.since.verify()?;
        let cycles = self.script.verify(max_cycles)?;
        let fee = self.fee_calculator.transaction_fee()?;
        Ok(CacheEntry::new(cycles, fee))
    }
}

pub struct FeeCalculator<'a, CS> {
    transaction: &'a ResolvedTransaction,
    consensus: &'a Consensus,
    chain_store: &'a CS,
}

impl<'a, CS: ChainStore<'a>> FeeCalculator<'a, CS> {
    fn new(
        transaction: &'a ResolvedTransaction,
        consensus: &'a Consensus,
        chain_store: &'a CS,
    ) -> Self {
        Self {
            transaction,
            consensus,
            chain_store,
        }
    }

    fn transaction_fee(&self) -> Result<Capacity, Error> {
        // skip tx fee calculation for cellbase
        if self.transaction.is_cellbase() {
            Ok(Capacity::zero())
        } else {
            DaoCalculator::new(&self.consensus, self.chain_store).transaction_fee(&self.transaction)
        }
    }
}

pub struct VersionVerifier<'a> {
    transaction: &'a TransactionView,
    tx_version: Version,
}

impl<'a> VersionVerifier<'a> {
    pub fn new(transaction: &'a TransactionView, tx_version: Version) -> Self {
        VersionVerifier {
            transaction,
            tx_version,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.transaction.version() != self.tx_version {
            return Err((TransactionError::MismatchedVersion).into());
        }
        Ok(())
    }
}

pub struct SizeVerifier<'a> {
    transaction: &'a TransactionView,
    block_bytes_limit: u64,
}

impl<'a> SizeVerifier<'a> {
    pub fn new(transaction: &'a TransactionView, block_bytes_limit: u64) -> Self {
        SizeVerifier {
            transaction,
            block_bytes_limit,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let size = self.transaction.data().serialized_size_in_block() as u64;
        if size <= self.block_bytes_limit {
            Ok(())
        } else {
            Err(TransactionError::ExceededMaximumBlockBytes.into())
        }
    }
}

pub struct ScriptVerifier<'a, CS> {
    chain_store: &'a CS,
    resolved_transaction: &'a ResolvedTransaction,
}

impl<'a, CS: ChainStore<'a>> ScriptVerifier<'a, CS> {
    pub fn new(resolved_transaction: &'a ResolvedTransaction, chain_store: &'a CS) -> Self {
        ScriptVerifier {
            chain_store,
            resolved_transaction,
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, Error> {
        let data_loader = DataLoaderWrapper::new(self.chain_store);
        TransactionScriptsVerifier::new(&self.resolved_transaction, &data_loader).verify(max_cycles)
    }
}

pub struct EmptyVerifier<'a> {
    transaction: &'a TransactionView,
}

impl<'a> EmptyVerifier<'a> {
    pub fn new(transaction: &'a TransactionView) -> Self {
        EmptyVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.transaction.inputs().is_empty() {
            Err(TransactionError::Empty {
                source: TransactionErrorSource::Inputs,
            }
            .into())
        } else if self.transaction.outputs().is_empty() && !self.transaction.is_cellbase() {
            Err(TransactionError::Empty {
                source: TransactionErrorSource::Outputs,
            }
            .into())
        } else {
            Ok(())
        }
    }
}

pub struct MaturityVerifier<'a> {
    transaction: &'a ResolvedTransaction,
    epoch: EpochNumberWithFraction,
    cellbase_maturity: EpochNumberWithFraction,
}

impl<'a> MaturityVerifier<'a> {
    pub fn new(
        transaction: &'a ResolvedTransaction,
        epoch: EpochNumberWithFraction,
        cellbase_maturity: EpochNumberWithFraction,
    ) -> Self {
        MaturityVerifier {
            transaction,
            epoch,
            cellbase_maturity,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let cellbase_immature = |meta: &CellMeta| -> bool {
            meta.transaction_info
                .as_ref()
                .map(|info| {
                    info.block_number > 0 && info.is_cellbase() && {
                        let threshold =
                            self.cellbase_maturity.to_rational() + info.block_epoch.to_rational();
                        let current = self.epoch.to_rational();
                        current < threshold
                    }
                })
                .unwrap_or(false)
        };

        if let Some(index) = self
            .transaction
            .resolved_inputs
            .iter()
            .position(cellbase_immature)
        {
            return Err(TransactionError::CellbaseImmaturity {
                source: TransactionErrorSource::Inputs,
                index,
            }
            .into());
        }

        if let Some(index) = self
            .transaction
            .resolved_cell_deps
            .iter()
            .position(cellbase_immature)
        {
            return Err(TransactionError::CellbaseImmaturity {
                source: TransactionErrorSource::CellDeps,
                index,
            }
            .into());
        }

        Ok(())
    }
}

pub struct DuplicateDepsVerifier<'a> {
    transaction: &'a TransactionView,
}

impl<'a> DuplicateDepsVerifier<'a> {
    pub fn new(transaction: &'a TransactionView) -> Self {
        DuplicateDepsVerifier { transaction }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let transaction = self.transaction;
        let mut seen_cells = HashSet::with_capacity(self.transaction.cell_deps().len());
        let mut seen_headers = HashSet::with_capacity(self.transaction.header_deps().len());

        if transaction
            .cell_deps_iter()
            .all(|dep| seen_cells.insert(dep))
            && transaction
                .header_deps_iter()
                .all(|hash| seen_headers.insert(hash))
        {
            Ok(())
        } else {
            Err(TransactionError::DuplicateDeps.into())
        }
    }
}

pub struct CapacityVerifier<'a> {
    resolved_transaction: &'a ResolvedTransaction,
    // It's Option because special genesis block do not have dao system cell
    dao_type_hash: Option<Byte32>,
}

impl<'a> CapacityVerifier<'a> {
    pub fn new(
        resolved_transaction: &'a ResolvedTransaction,
        dao_type_hash: Option<Byte32>,
    ) -> Self {
        CapacityVerifier {
            resolved_transaction,
            dao_type_hash,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        // skip OutputsSumOverflow verification for resolved cellbase and DAO
        // withdraw transactions.
        // cellbase's outputs are verified by RewardVerifier
        // DAO withdraw transaction is verified via the type script of DAO cells
        if !(self.resolved_transaction.is_cellbase() || self.valid_dao_withdraw_transaction()) {
            let inputs_sum = self.resolved_transaction.inputs_capacity()?;
            let outputs_sum = self.resolved_transaction.outputs_capacity()?;

            if inputs_sum < outputs_sum {
                return Err((TransactionError::OutputsSumOverflow {
                    inputs_sum,
                    outputs_sum,
                })
                .into());
            }
        }

        for (index, (output, data)) in self
            .resolved_transaction
            .transaction
            .outputs_with_data_iter()
            .enumerate()
        {
            let data_occupied_capacity = Capacity::bytes(data.len())?;
            if output.is_lack_of_capacity(data_occupied_capacity)? {
                return Err((TransactionError::InsufficientCellCapacity {
                    index,
                    source: TransactionErrorSource::Outputs,
                    capacity: output.capacity().unpack(),
                    occupied_capacity: output.occupied_capacity(data_occupied_capacity)?,
                })
                .into());
            }
        }

        Ok(())
    }

    fn valid_dao_withdraw_transaction(&self) -> bool {
        self.resolved_transaction
            .resolved_inputs
            .iter()
            .any(|cell_meta| {
                cell_meta
                    .cell_output
                    .type_()
                    .to_opt()
                    .map(|t| {
                        Into::<u8>::into(t.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
                            && &t.code_hash()
                                == self.dao_type_hash.as_ref().expect("No dao system cell")
                    })
                    .unwrap_or(false)
            })
    }
}

const LOCK_TYPE_FLAG: u64 = 1 << 63;
const METRIC_TYPE_FLAG_MASK: u64 = 0x6000_0000_0000_0000;
const VALUE_MASK: u64 = 0x00ff_ffff_ffff_ffff;
const REMAIN_FLAGS_BITS: u64 = 0x1f00_0000_0000_0000;

pub enum SinceMetric {
    BlockNumber(u64),
    EpochNumberWithFraction(EpochNumberWithFraction),
    Timestamp(u64),
}

/// RFC 0017
#[derive(Copy, Clone, Debug)]
pub struct Since(pub u64);

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

    pub fn extract_metric(self) -> Option<SinceMetric> {
        let value = self.0 & VALUE_MASK;
        match self.0 & METRIC_TYPE_FLAG_MASK {
            //0b0000_0000
            0x0000_0000_0000_0000 => Some(SinceMetric::BlockNumber(value)),
            //0b0010_0000
            0x2000_0000_0000_0000 => Some(SinceMetric::EpochNumberWithFraction(
                EpochNumberWithFraction::from_full_value(value),
            )),
            //0b0100_0000
            0x4000_0000_0000_0000 => Some(SinceMetric::Timestamp(value * 1000)),
            _ => None,
        }
    }
}

/// https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md#detailed-specification
pub struct SinceVerifier<'a, M> {
    rtx: &'a ResolvedTransaction,
    block_median_time_context: &'a M,
    block_number: BlockNumber,
    epoch_number_with_fraction: EpochNumberWithFraction,
    parent_hash: Byte32,
    median_timestamps_cache: RefCell<LruCache<Byte32, u64>>,
}

impl<'a, M> SinceVerifier<'a, M>
where
    M: BlockMedianTimeContext,
{
    pub fn new(
        rtx: &'a ResolvedTransaction,
        block_median_time_context: &'a M,
        block_number: BlockNumber,
        epoch_number_with_fraction: EpochNumberWithFraction,
        parent_hash: Byte32,
    ) -> Self {
        let median_timestamps_cache = RefCell::new(LruCache::new(rtx.resolved_inputs.len()));
        SinceVerifier {
            rtx,
            block_median_time_context,
            block_number,
            epoch_number_with_fraction,
            parent_hash,
            median_timestamps_cache,
        }
    }

    fn parent_median_time(&self, block_hash: &Byte32) -> u64 {
        let (_, _, parent_hash) = self
            .block_median_time_context
            .timestamp_and_parent(block_hash);
        self.block_median_time(&parent_hash)
    }

    fn block_median_time(&self, block_hash: &Byte32) -> u64 {
        if let Some(median_time) = self.median_timestamps_cache.borrow().get(block_hash) {
            return *median_time;
        }

        let median_time = self.block_median_time_context.block_median_time(block_hash);
        self.median_timestamps_cache
            .borrow_mut()
            .insert(block_hash.clone(), median_time);
        median_time
    }

    fn verify_absolute_lock(&self, since: Since) -> Result<(), Error> {
        if since.is_absolute() {
            match since.extract_metric() {
                Some(SinceMetric::BlockNumber(block_number)) => {
                    if self.block_number < block_number {
                        return Err((TransactionError::Immature).into());
                    }
                }
                Some(SinceMetric::EpochNumberWithFraction(epoch_number_with_fraction)) => {
                    if self.epoch_number_with_fraction < epoch_number_with_fraction {
                        return Err((TransactionError::Immature).into());
                    }
                }
                Some(SinceMetric::Timestamp(timestamp)) => {
                    let tip_timestamp = self.block_median_time(&self.parent_hash);
                    if tip_timestamp < timestamp {
                        return Err((TransactionError::Immature).into());
                    }
                }
                None => {
                    return Err((TransactionError::InvalidSince).into());
                }
            }
        }
        Ok(())
    }

    fn verify_relative_lock(&self, since: Since, cell_meta: &CellMeta) -> Result<(), Error> {
        if since.is_relative() {
            let info = match cell_meta.transaction_info {
                Some(ref transaction_info) => Ok(transaction_info),
                None => Err(TransactionError::Immature),
            }?;
            match since.extract_metric() {
                Some(SinceMetric::BlockNumber(block_number)) => {
                    if self.block_number < info.block_number + block_number {
                        return Err((TransactionError::Immature).into());
                    }
                }
                Some(SinceMetric::EpochNumberWithFraction(epoch_number_with_fraction)) => {
                    let a = self.epoch_number_with_fraction.to_rational();
                    let b =
                        info.block_epoch.to_rational() + epoch_number_with_fraction.to_rational();
                    if a < b {
                        return Err((TransactionError::Immature).into());
                    }
                }
                Some(SinceMetric::Timestamp(timestamp)) => {
                    // pass_median_time(current_block) starts with tip block, which is the
                    // parent of current block.
                    // pass_median_time(input_cell's block) starts with cell_block_number - 1,
                    // which is the parent of input_cell's block
                    let cell_median_timestamp = self.parent_median_time(&info.block_hash);
                    let current_median_time = self.block_median_time(&self.parent_hash);
                    if current_median_time < cell_median_timestamp + timestamp {
                        return Err((TransactionError::Immature).into());
                    }
                }
                None => {
                    return Err((TransactionError::InvalidSince).into());
                }
            }
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<(), Error> {
        for (cell_meta, input) in self
            .rtx
            .resolved_inputs
            .iter()
            .zip(self.rtx.transaction.inputs())
        {
            // ignore empty since
            let since: u64 = input.since().unpack();
            if since == 0 {
                continue;
            }
            let since = Since(since);
            // check remain flags
            if !since.flags_is_valid() {
                return Err((TransactionError::InvalidSince).into());
            }

            // verify time lock
            self.verify_absolute_lock(since)?;
            self.verify_relative_lock(since, cell_meta)?;
        }
        Ok(())
    }
}

pub struct OutputsDataVerifier<'a> {
    transaction: &'a TransactionView,
}

impl<'a> OutputsDataVerifier<'a> {
    pub fn new(transaction: &'a TransactionView) -> Self {
        Self { transaction }
    }

    pub fn verify(&self) -> Result<(), TransactionError> {
        if self.transaction.outputs().len() != self.transaction.outputs_data().len() {
            return Err(TransactionError::OutputsDataLengthMismatch);
        }
        Ok(())
    }
}
