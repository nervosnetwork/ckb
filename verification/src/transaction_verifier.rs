use crate::cache::Completed;
use crate::error::TransactionErrorSource;
use crate::{TransactionError, TxVerifyEnv};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_dao_utils::DaoError;
use ckb_error::Error;
use ckb_script::{TransactionScriptsVerifier, TransactionSnapshot, TransactionState, VerifyResult};
use ckb_traits::{CellDataProvider, EpochProvider, HeaderFieldsProvider, HeaderProvider};
use ckb_types::{
    core::{
        cell::{CellMeta, ResolvedTransaction},
        Capacity, Cycle, EpochNumberWithFraction, ScriptHashType, TransactionView, Version,
    },
    packed::Byte32,
    prelude::*,
};
use std::collections::HashSet;
use std::sync::Arc;

/// The time-related TX verification
///
/// Contains:
/// [`MaturityVerifier`](./struct.MaturityVerifier.html)
/// [`SinceVerifier`](./struct.SinceVerifier.html)
pub struct TimeRelativeTransactionVerifier<'a, M> {
    pub(crate) maturity: MaturityVerifier,
    pub(crate) since: SinceVerifier<'a, M>,
}

impl<'a, DL: HeaderFieldsProvider> TimeRelativeTransactionVerifier<'a, DL> {
    /// Creates a new TimeRelativeTransactionVerifier
    pub fn new(
        rtx: Arc<ResolvedTransaction>,
        consensus: &'a Consensus,
        data_loader: DL,
        tx_env: &'a TxVerifyEnv,
    ) -> Self {
        TimeRelativeTransactionVerifier {
            maturity: MaturityVerifier::new(
                Arc::clone(&rtx),
                tx_env.epoch(),
                consensus.cellbase_maturity(),
            ),
            since: SinceVerifier::new(rtx, consensus, data_loader, tx_env),
        }
    }

    /// Perform time-related verification
    pub fn verify(&self) -> Result<(), Error> {
        self.maturity.verify()?;
        self.since.verify()?;
        Ok(())
    }
}

/// Context-independent verification checks for transaction
///
/// Basic checks that don't depend on any context
/// Contains:
/// - Check for version
/// - Check for size
/// - Check inputs and output empty
/// - Check for duplicate deps
/// - Check for whether outputs match data
pub struct NonContextualTransactionVerifier<'a> {
    pub(crate) version: VersionVerifier<'a>,
    pub(crate) size: SizeVerifier<'a>,
    pub(crate) empty: EmptyVerifier<'a>,
    pub(crate) duplicate_deps: DuplicateDepsVerifier<'a>,
    pub(crate) outputs_data_verifier: OutputsDataVerifier<'a>,
}

impl<'a> NonContextualTransactionVerifier<'a> {
    /// Creates a new NonContextualTransactionVerifier
    pub fn new(tx: &'a TransactionView, consensus: &'a Consensus) -> Self {
        NonContextualTransactionVerifier {
            version: VersionVerifier::new(tx, consensus.tx_version()),
            size: SizeVerifier::new(tx, consensus.max_block_bytes()),
            empty: EmptyVerifier::new(tx),
            duplicate_deps: DuplicateDepsVerifier::new(tx),
            outputs_data_verifier: OutputsDataVerifier::new(tx),
        }
    }

    /// Perform context-independent verification
    pub fn verify(&self) -> Result<(), Error> {
        self.version.verify()?;
        self.size.verify()?;
        self.empty.verify()?;
        self.duplicate_deps.verify()?;
        self.outputs_data_verifier.verify()?;
        Ok(())
    }
}

/// Context-dependent verification checks for transaction
///
/// Contains:
/// [`TimeRelativeTransactionVerifier`](./struct.TimeRelativeTransactionVerifier.html)
/// [`CapacityVerifier`](./struct.CapacityVerifier.html)
/// [`ScriptVerifier`](./struct.ScriptVerifier.html)
/// [`FeeCalculator`](./struct.FeeCalculator.html)
pub struct ContextualTransactionVerifier<'a, DL> {
    pub(crate) time_relative: TimeRelativeTransactionVerifier<'a, DL>,
    pub(crate) capacity: CapacityVerifier,
    pub(crate) script: ScriptVerifier<DL>,
    pub(crate) fee_calculator: FeeCalculator<'a, DL>,
}

impl<'a, DL> ContextualTransactionVerifier<'a, DL>
where
    DL: CellDataProvider
        + HeaderProvider
        + HeaderFieldsProvider
        + EpochProvider
        + Send
        + Sync
        + Clone
        + 'static,
{
    /// Creates a new ContextualTransactionVerifier
    pub fn new(
        rtx: Arc<ResolvedTransaction>,
        consensus: &'a Consensus,
        data_loader: DL,
        tx_env: &'a TxVerifyEnv,
    ) -> Self {
        ContextualTransactionVerifier {
            time_relative: TimeRelativeTransactionVerifier::new(
                Arc::clone(&rtx),
                consensus,
                data_loader.clone(),
                tx_env,
            ),
            script: ScriptVerifier::new(Arc::clone(&rtx), data_loader.clone()),
            capacity: CapacityVerifier::new(Arc::clone(&rtx), consensus.dao_type_hash()),
            fee_calculator: FeeCalculator::new(rtx, consensus, data_loader),
        }
    }

    /// Perform resumable context-dependent verification, return a `Result` to `CacheEntry`
    pub fn resumable_verify(&self, limit_cycles: Cycle) -> Result<(VerifyResult, Capacity), Error> {
        self.time_relative.verify()?;
        self.capacity.verify()?;
        let fee = self.fee_calculator.transaction_fee()?;
        let ret = self.script.resumable_verify(limit_cycles)?;
        Ok((ret, fee))
    }

    /// Perform context-dependent verification, return a `Result` to `CacheEntry`
    ///
    /// skip script verify will result in the return value cycle always is zero
    pub fn verify(&self, max_cycles: Cycle, skip_script_verify: bool) -> Result<Completed, Error> {
        self.time_relative.verify()?;
        self.capacity.verify()?;
        let cycles = if skip_script_verify {
            0
        } else {
            self.script.verify(max_cycles)?
        };
        let fee = self.fee_calculator.transaction_fee()?;
        Ok(Completed { cycles, fee })
    }

    /// Perform complete a suspend context-dependent verification, return a `Result` to `CacheEntry`
    ///
    /// skip script verify will result in the return value cycle always is zero
    pub fn complete(
        &self,
        max_cycles: Cycle,
        skip_script_verify: bool,
        snapshot: &TransactionSnapshot,
    ) -> Result<Completed, Error> {
        self.time_relative.verify()?;
        self.capacity.verify()?;
        let cycles = if skip_script_verify {
            0
        } else {
            self.script.complete(snapshot, max_cycles)?
        };
        let fee = self.fee_calculator.transaction_fee()?;
        Ok(Completed { cycles, fee })
    }
}

// /// Full tx verification checks
// ///
// /// Contains:
// /// [`NonContextualTransactionVerifier`](./struct.NonContextualTransactionVerifier.html)
// /// [`ContextualTransactionVerifier`](./struct.ContextualTransactionVerifier.html)
// pub struct TransactionVerifier<'a, DL> {
//     pub(crate) non_contextual: NonContextualTransactionVerifier<'a>,
//     pub(crate) contextual: ContextualTransactionVerifier<'a, DL>,
// }

// impl<'a, DL: HeaderProvider + CellDataProvider + EpochProvider + Send + Sync + Clone + 'static>
//     TransactionVerifier<'a, DL>
// {
//     /// Creates a new TransactionVerifier
//     pub fn new(
//         rtx: Arc<ResolvedTransaction>,
//         consensus: &'a Consensus,
//         data_loader: DL,
//         tx_env: &'a TxVerifyEnv,
//     ) -> Self {
//         TransactionVerifier {
//             non_contextual: NonContextualTransactionVerifier::new(&rtx.transaction, consensus),
//             contextual: ContextualTransactionVerifier::new(rtx, consensus, data_loader, tx_env),
//         }
//     }

//     /// Perform all tx verification
//     pub fn verify(&self, max_cycles: Cycle) -> Result<Completed, Error> {
//         self.non_contextual.verify()?;
//         self.contextual.verify(max_cycles, false)
//     }
// }

pub struct FeeCalculator<'a, DL> {
    transaction: Arc<ResolvedTransaction>,
    consensus: &'a Consensus,
    data_loader: DL,
}

impl<'a, DL: CellDataProvider + HeaderProvider + EpochProvider> FeeCalculator<'a, DL> {
    fn new(
        transaction: Arc<ResolvedTransaction>,
        consensus: &'a Consensus,
        data_loader: DL,
    ) -> Self {
        Self {
            transaction,
            consensus,
            data_loader,
        }
    }

    fn transaction_fee(&self) -> Result<Capacity, DaoError> {
        // skip tx fee calculation for cellbase
        if self.transaction.is_cellbase() {
            Ok(Capacity::zero())
        } else {
            DaoCalculator::new(self.consensus, &self.data_loader).transaction_fee(&self.transaction)
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
            return Err((TransactionError::MismatchedVersion {
                expected: self.tx_version,
                actual: self.transaction.version(),
            })
            .into());
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
            Err(TransactionError::ExceededMaximumBlockBytes {
                actual: size,
                limit: self.block_bytes_limit,
            }
            .into())
        }
    }
}

/// Perform rules verification describe in CKB script, also check cycles limit
///
/// See:
/// - [ckb-vm](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0003-ckb-vm/0003-ckb-vm.md)
/// - [vm-cycle-limits](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0014-vm-cycle-limits/0014-vm-cycle-limits.md)
pub struct ScriptVerifier<DL> {
    inner: TransactionScriptsVerifier<DL>,
}

impl<DL: CellDataProvider + HeaderProvider + Send + Sync + Clone + 'static> ScriptVerifier<DL> {
    /// Creates a new ScriptVerifier
    pub fn new(resolved_transaction: Arc<ResolvedTransaction>, data_loader: DL) -> Self {
        ScriptVerifier {
            inner: TransactionScriptsVerifier::new(resolved_transaction, data_loader),
        }
    }

    /// Perform script verification
    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, Error> {
        let cycle = self.inner.verify(max_cycles)?;
        Ok(cycle)
    }

    /// Perform resumable script verification
    pub fn resumable_verify(&self, limit_cycles: Cycle) -> Result<VerifyResult, Error> {
        let ret = self.inner.resumable_verify(limit_cycles)?;
        Ok(ret)
    }

    /// Perform verification resume from snapshot
    pub fn resume_from_snap(
        &self,
        snapshot: &TransactionSnapshot,
        limit_cycles: Cycle,
    ) -> Result<VerifyResult, Error> {
        let ret = self.inner.resume_from_snap(snapshot, limit_cycles)?;
        Ok(ret)
    }

    /// Perform verification resume from snapshot
    pub fn resume_from_state(
        &self,
        state: TransactionState,
        limit_cycles: Cycle,
    ) -> Result<VerifyResult, Error> {
        let ret = self.inner.resume_from_state(state, limit_cycles)?;
        Ok(ret)
    }

    /// Perform complete verification
    pub fn complete(
        &self,
        snapshot: &TransactionSnapshot,
        max_cycles: Cycle,
    ) -> Result<Cycle, Error> {
        let ret = self.inner.complete(snapshot, max_cycles)?;
        Ok(ret)
    }

    /// Explicitly dereferencing operation
    pub fn inner(&self) -> &TransactionScriptsVerifier<DL> {
        &self.inner
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
                inner: TransactionErrorSource::Inputs,
            }
            .into())
        } else if self.transaction.outputs().is_empty() && !self.transaction.is_cellbase() {
            Err(TransactionError::Empty {
                inner: TransactionErrorSource::Outputs,
            }
            .into())
        } else {
            Ok(())
        }
    }
}

/// MaturityVerifier
///
/// If input or dep prev is cellbase, check that it's matured
pub struct MaturityVerifier {
    transaction: Arc<ResolvedTransaction>,
    epoch: EpochNumberWithFraction,
    cellbase_maturity: EpochNumberWithFraction,
}

impl MaturityVerifier {
    pub fn new(
        transaction: Arc<ResolvedTransaction>,
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
                inner: TransactionErrorSource::Inputs,
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
                inner: TransactionErrorSource::CellDeps,
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

        if let Some(dep) = transaction
            .cell_deps_iter()
            .find_map(|dep| seen_cells.replace(dep))
        {
            return Err(TransactionError::DuplicateCellDeps {
                out_point: dep.out_point(),
            }
            .into());
        }
        if let Some(hash) = transaction
            .header_deps_iter()
            .find_map(|hash| seen_headers.replace(hash))
        {
            return Err(TransactionError::DuplicateHeaderDeps { hash }.into());
        }
        Ok(())
    }
}

/// Perform inputs and outputs `capacity` field related verification
pub struct CapacityVerifier {
    resolved_transaction: Arc<ResolvedTransaction>,
    // It's Option because special genesis block do not have dao system cell
    dao_type_hash: Option<Byte32>,
}

impl CapacityVerifier {
    /// Create a new `CapacityVerifier`
    pub fn new(
        resolved_transaction: Arc<ResolvedTransaction>,
        dao_type_hash: Option<Byte32>,
    ) -> Self {
        CapacityVerifier {
            resolved_transaction,
            dao_type_hash,
        }
    }

    /// Verify sum of inputs capacity should be greater than or equal to sum of outputs capacity
    /// Verify outputs capacity should be greater than or equal to its occupied capacity
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
                    inner: TransactionErrorSource::Outputs,
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

/// Metric represent value
pub enum SinceMetric {
    /// The metric_flag is 00, `value` can be explained as a block number or a relative block number.
    BlockNumber(u64),
    /// The metric_flag is 01, value can be explained as an absolute epoch or relative epoch.
    EpochNumberWithFraction(EpochNumberWithFraction),
    /// The metric_flag is 10, value can be explained as a block timestamp(unix time) or a relative
    Timestamp(u64),
}

/// The struct define wrapper for (unsigned 64-bit integer) tx field since
///
/// See [tx-since](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md)
#[derive(Copy, Clone, Debug)]
pub struct Since(pub u64);

impl Since {
    /// Whether since represented absolute form
    pub fn is_absolute(self) -> bool {
        self.0 & LOCK_TYPE_FLAG == 0
    }

    /// Whether since represented relative form
    #[inline]
    pub fn is_relative(self) -> bool {
        !self.is_absolute()
    }

    /// Whether since flag is valid
    pub fn flags_is_valid(self) -> bool {
        (self.0 & REMAIN_FLAGS_BITS == 0)
            && ((self.0 & METRIC_TYPE_FLAG_MASK) != METRIC_TYPE_FLAG_MASK)
    }

    /// Extracts a `SinceMetric` from an unsigned 64-bit integer since
    pub fn extract_metric(self) -> Option<SinceMetric> {
        let value = self.0 & VALUE_MASK;
        match self.0 & METRIC_TYPE_FLAG_MASK {
            //0b0000_0000
            0x0000_0000_0000_0000 => Some(SinceMetric::BlockNumber(value)),
            //0b0010_0000
            0x2000_0000_0000_0000 => Some(SinceMetric::EpochNumberWithFraction(
                EpochNumberWithFraction::from_full_value_unchecked(value),
            )),
            //0b0100_0000
            0x4000_0000_0000_0000 => Some(SinceMetric::Timestamp(value * 1000)),
            _ => None,
        }
    }
}

/// SinceVerifier
///
/// Rules detail see:
/// [tx-since-specification](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md#detailed-specification
pub struct SinceVerifier<'a, DL> {
    rtx: Arc<ResolvedTransaction>,
    consensus: &'a Consensus,
    data_loader: DL,
    tx_env: &'a TxVerifyEnv,
}

impl<'a, DL: HeaderFieldsProvider> SinceVerifier<'a, DL> {
    pub fn new(
        rtx: Arc<ResolvedTransaction>,
        consensus: &'a Consensus,
        data_loader: DL,
        tx_env: &'a TxVerifyEnv,
    ) -> Self {
        SinceVerifier {
            rtx,
            consensus,
            data_loader,
            tx_env,
        }
    }

    fn parent_median_time(&self, block_hash: &Byte32) -> u64 {
        let header_fields = self
            .data_loader
            .get_header_fields(block_hash)
            .expect("parent block exist");
        self.block_median_time(&header_fields.parent_hash)
    }

    fn block_median_time(&self, block_hash: &Byte32) -> u64 {
        let median_block_count = self.consensus.median_time_block_count();
        self.data_loader
            .block_median_time(block_hash, median_block_count)
    }

    fn verify_absolute_lock(&self, index: usize, since: Since) -> Result<(), Error> {
        if since.is_absolute() {
            match since.extract_metric() {
                Some(SinceMetric::BlockNumber(block_number)) => {
                    let proposal_window = self.consensus.tx_proposal_window();
                    if self.tx_env.block_number(proposal_window) < block_number {
                        return Err((TransactionError::Immature { index }).into());
                    }
                }
                Some(SinceMetric::EpochNumberWithFraction(epoch_number_with_fraction)) => {
                    if !epoch_number_with_fraction.is_well_formed_increment() {
                        return Err((TransactionError::InvalidSince { index }).into());
                    }
                    let a = self.tx_env.epoch().to_rational();
                    let b = epoch_number_with_fraction.normalize().to_rational();
                    if a < b {
                        return Err((TransactionError::Immature { index }).into());
                    }
                }
                Some(SinceMetric::Timestamp(timestamp)) => {
                    let parent_hash = self.tx_env.parent_hash();
                    let tip_timestamp = self.block_median_time(&parent_hash);
                    if tip_timestamp < timestamp {
                        return Err((TransactionError::Immature { index }).into());
                    }
                }
                None => {
                    return Err((TransactionError::InvalidSince { index }).into());
                }
            }
        }
        Ok(())
    }

    fn verify_relative_lock(
        &self,
        index: usize,
        since: Since,
        cell_meta: &CellMeta,
    ) -> Result<(), Error> {
        if since.is_relative() {
            let info = match cell_meta.transaction_info {
                Some(ref transaction_info) => Ok(transaction_info),
                None => Err(TransactionError::Immature { index }),
            }?;
            match since.extract_metric() {
                Some(SinceMetric::BlockNumber(block_number)) => {
                    let proposal_window = self.consensus.tx_proposal_window();
                    if self.tx_env.block_number(proposal_window) < info.block_number + block_number
                    {
                        return Err((TransactionError::Immature { index }).into());
                    }
                }
                Some(SinceMetric::EpochNumberWithFraction(epoch_number_with_fraction)) => {
                    if !epoch_number_with_fraction.is_well_formed_increment() {
                        return Err((TransactionError::InvalidSince { index }).into());
                    }
                    let a = self.tx_env.epoch().to_rational();
                    let b = info.block_epoch.to_rational()
                        + epoch_number_with_fraction.normalize().to_rational();
                    if a < b {
                        return Err((TransactionError::Immature { index }).into());
                    }
                }
                Some(SinceMetric::Timestamp(timestamp)) => {
                    // pass_median_time(current_block) starts with tip block, which is the
                    // parent of current block.
                    // pass_median_time(input_cell's block) starts with cell_block_number - 1,
                    // which is the parent of input_cell's block
                    let proposal_window = self.consensus.tx_proposal_window();
                    let parent_hash = self.tx_env.parent_hash();
                    let epoch_number = self.tx_env.epoch_number(proposal_window);
                    let hardfork_switch = self.consensus.hardfork_switch();
                    let base_timestamp = if hardfork_switch
                        .is_block_ts_as_relative_since_start_enabled(epoch_number)
                    {
                        self.data_loader
                            .get_header_fields(&info.block_hash)
                            .expect("header exist")
                            .timestamp
                    } else {
                        self.parent_median_time(&info.block_hash)
                    };
                    let current_median_time = self.block_median_time(&parent_hash);
                    if current_median_time < base_timestamp + timestamp {
                        return Err((TransactionError::Immature { index }).into());
                    }
                }
                None => {
                    return Err((TransactionError::InvalidSince { index }).into());
                }
            }
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<(), Error> {
        for (index, (cell_meta, input)) in self
            .rtx
            .resolved_inputs
            .iter()
            .zip(self.rtx.transaction.inputs())
            .enumerate()
        {
            // ignore empty since
            let since: u64 = input.since().unpack();
            if since == 0 {
                continue;
            }
            let since = Since(since);
            // check remain flags
            if !since.flags_is_valid() {
                return Err((TransactionError::InvalidSince { index }).into());
            }

            // verify time lock
            self.verify_absolute_lock(index, since)?;
            self.verify_relative_lock(index, since, cell_meta)?;
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
        let outputs_len = self.transaction.outputs().len();
        let outputs_data_len = self.transaction.outputs_data().len();

        if outputs_len != outputs_data_len {
            return Err(TransactionError::OutputsDataLengthMismatch {
                outputs_len,
                outputs_data_len,
            });
        }
        Ok(())
    }
}

/// Context-dependent checks exclude script
///
/// Contains:
/// [`TimeRelativeTransactionVerifier`](./struct.TimeRelativeTransactionVerifier.html)
/// [`CapacityVerifier`](./struct.CapacityVerifier.html)
/// [`FeeCalculator`](./struct.FeeCalculator.html)
pub struct ContextualWithoutScriptTransactionVerifier<'a, DL> {
    pub(crate) time_relative: TimeRelativeTransactionVerifier<'a, DL>,
    pub(crate) capacity: CapacityVerifier,
    pub(crate) fee_calculator: FeeCalculator<'a, DL>,
}

impl<'a, DL> ContextualWithoutScriptTransactionVerifier<'a, DL>
where
    DL: CellDataProvider
        + HeaderProvider
        + HeaderFieldsProvider
        + EpochProvider
        + Send
        + Sync
        + Clone
        + 'static,
{
    /// Creates a new ContextualWithoutScriptTransactionVerifier
    pub fn new(
        rtx: Arc<ResolvedTransaction>,
        consensus: &'a Consensus,
        data_loader: DL,
        tx_env: &'a TxVerifyEnv,
    ) -> Self {
        ContextualWithoutScriptTransactionVerifier {
            time_relative: TimeRelativeTransactionVerifier::new(
                Arc::clone(&rtx),
                consensus,
                data_loader.clone(),
                tx_env,
            ),
            capacity: CapacityVerifier::new(Arc::clone(&rtx), consensus.dao_type_hash()),
            fee_calculator: FeeCalculator::new(rtx, consensus, data_loader),
        }
    }

    /// Perform verification
    pub fn verify(&self) -> Result<Capacity, Error> {
        self.time_relative.verify()?;
        self.capacity.verify()?;
        let fee = self.fee_calculator.transaction_fee()?;
        Ok(fee)
    }
}
