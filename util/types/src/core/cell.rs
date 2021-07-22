//! TODO(doc): @quake
use crate::{
    bytes::Bytes,
    core::error::OutPointError,
    core::{BlockView, Capacity, DepType, TransactionInfo, TransactionView},
    packed::{Byte32, CellDep, CellOutput, CellOutputVec, OutPoint, OutPointVec},
    prelude::*,
};
use bitflags::bitflags;
use ckb_error::Error;
use ckb_occupied_capacity::Result as CapacityResult;
use once_cell::sync::OnceCell;
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::convert::TryInto;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};

/// TODO(doc): @quake
#[derive(Debug)]
pub enum ResolvedDep {
    /// TODO(doc): @quake
    Cell(CellMeta),
    /// TODO(doc): @quake
    Group((CellMeta, Vec<CellMeta>)),
}

/// type alias system cells map
pub type SystemCellMap = HashMap<CellDep, ResolvedDep>;
/// system cell memory map cache
pub static SYSTEM_CELL: OnceCell<SystemCellMap> = OnceCell::new();

/// TODO(doc): @quake
#[derive(Clone, Eq, PartialEq, Default)]
pub struct CellMeta {
    /// TODO(doc): @quake
    pub cell_output: CellOutput,
    /// TODO(doc): @quake
    pub out_point: OutPoint,
    /// TODO(doc): @quake
    pub transaction_info: Option<TransactionInfo>,
    /// TODO(doc): @quake
    pub data_bytes: u64,
    /// In memory cell data
    /// A live cell either exists in memory or DB
    /// must check DB if this field is None
    pub mem_cell_data: Option<Bytes>,
    /// memory cell data hash
    /// A live cell either exists in memory or DB
    /// must check DB if this field is None
    pub mem_cell_data_hash: Option<Byte32>,
}

/// TODO(doc): @quake
#[derive(Default)]
pub struct CellMetaBuilder {
    cell_output: CellOutput,
    out_point: OutPoint,
    transaction_info: Option<TransactionInfo>,
    data_bytes: u64,
    mem_cell_data: Option<Bytes>,
    mem_cell_data_hash: Option<Byte32>,
}

impl CellMetaBuilder {
    /// TODO(doc): @quake
    pub fn from_cell_meta(cell_meta: CellMeta) -> Self {
        let CellMeta {
            cell_output,
            out_point,
            transaction_info,
            data_bytes,
            mem_cell_data,
            mem_cell_data_hash,
        } = cell_meta;
        Self {
            cell_output,
            out_point,
            transaction_info,
            data_bytes,
            mem_cell_data,
            mem_cell_data_hash,
        }
    }

    /// TODO(doc): @quake
    pub fn from_cell_output(cell_output: CellOutput, data: Bytes) -> Self {
        CellMetaBuilder {
            cell_output,
            data_bytes: data.len().try_into().expect("u32"),
            mem_cell_data_hash: Some(CellOutput::calc_data_hash(&data)),
            mem_cell_data: Some(data),
            ..Default::default()
        }
    }

    /// TODO(doc): @quake
    pub fn out_point(mut self, out_point: OutPoint) -> Self {
        self.out_point = out_point;
        self
    }

    /// TODO(doc): @quake
    pub fn transaction_info(mut self, transaction_info: TransactionInfo) -> Self {
        self.transaction_info = Some(transaction_info);
        self
    }

    /// TODO(doc): @quake
    pub fn build(self) -> CellMeta {
        let Self {
            cell_output,
            out_point,
            transaction_info,
            data_bytes,
            mem_cell_data,
            mem_cell_data_hash,
        } = self;
        CellMeta {
            cell_output,
            out_point,
            transaction_info,
            data_bytes,
            mem_cell_data,
            mem_cell_data_hash,
        }
    }
}

impl fmt::Debug for CellMeta {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CellMeta")
            .field("cell_output", &self.cell_output)
            .field("out_point", &self.out_point)
            .field("transaction_info", &self.transaction_info)
            .field("data_bytes", &self.data_bytes)
            .finish()
    }
}

impl CellMeta {
    /// TODO(doc): @quake
    pub fn is_cellbase(&self) -> bool {
        self.transaction_info
            .as_ref()
            .map(TransactionInfo::is_cellbase)
            .unwrap_or(false)
    }

    /// TODO(doc): @quake
    pub fn capacity(&self) -> Capacity {
        self.cell_output.capacity().unpack()
    }

    /// TODO(doc): @quake
    pub fn occupied_capacity(&self) -> CapacityResult<Capacity> {
        self.cell_output
            .occupied_capacity(Capacity::bytes(self.data_bytes as usize)?)
    }

    /// TODO(doc): @quake
    pub fn is_lack_of_capacity(&self) -> CapacityResult<bool> {
        self.cell_output
            .is_lack_of_capacity(Capacity::bytes(self.data_bytes as usize)?)
    }
}

/// TODO(doc): @quake
#[derive(PartialEq, Debug)]
pub enum CellStatus {
    /// Cell exists and has not been spent.
    Live(CellMeta),
    /// Cell exists and has been spent.
    Dead,
    /// Cell is out of index.
    Unknown,
}

impl CellStatus {
    /// TODO(doc): @quake
    pub fn live_cell(cell_meta: CellMeta) -> CellStatus {
        CellStatus::Live(cell_meta)
    }

    /// TODO(doc): @quake
    pub fn is_live(&self) -> bool {
        matches!(*self, CellStatus::Live(_))
    }

    /// TODO(doc): @quake
    pub fn is_dead(&self) -> bool {
        self == &CellStatus::Dead
    }

    /// Returns true if the status is a Unknown value.
    pub fn is_unknown(&self) -> bool {
        self == &CellStatus::Unknown
    }
}

/// Transaction with resolved input cells.
#[derive(Debug, Clone, Eq)]
pub struct ResolvedTransaction {
    /// TODO(doc): @quake
    pub transaction: TransactionView,
    /// TODO(doc): @quake
    pub resolved_cell_deps: Vec<CellMeta>,
    /// TODO(doc): @quake
    pub resolved_inputs: Vec<CellMeta>,
    /// TODO(doc): @quake
    pub resolved_dep_groups: Vec<CellMeta>,
}

impl Hash for ResolvedTransaction {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.transaction, state);
    }
}

impl PartialEq for ResolvedTransaction {
    fn eq(&self, other: &ResolvedTransaction) -> bool {
        self.transaction == other.transaction
    }
}

impl ResolvedTransaction {
    /// Construct `ResolvedTransaction` from `TransactionView` without actually performing resolve
    pub fn dummy_resolve(tx: TransactionView) -> Self {
        ResolvedTransaction {
            transaction: tx,
            resolved_cell_deps: vec![],
            resolved_inputs: vec![],
            resolved_dep_groups: vec![],
        }
    }

    /// TODO(doc): @quake
    // cellbase will be resolved with empty input cells, we can use low cost check here:
    pub fn is_cellbase(&self) -> bool {
        self.resolved_inputs.is_empty()
    }

    /// TODO(doc): @quake
    pub fn inputs_capacity(&self) -> CapacityResult<Capacity> {
        self.resolved_inputs
            .iter()
            .map(CellMeta::capacity)
            .try_fold(Capacity::zero(), Capacity::safe_add)
    }

    /// TODO(doc): @quake
    pub fn outputs_capacity(&self) -> CapacityResult<Capacity> {
        self.transaction.outputs_capacity()
    }

    /// TODO(doc): @quake
    pub fn related_dep_out_points(&self) -> impl Iterator<Item = &OutPoint> {
        self.resolved_cell_deps
            .iter()
            .map(|d| &d.out_point)
            .chain(self.resolved_dep_groups.iter().map(|d| &d.out_point))
    }

    /// Check if all inputs and deps are still valid
    pub fn check<CC: CellChecker, HC: HeaderChecker, S: BuildHasher>(
        &self,
        seen_inputs: &mut HashSet<OutPoint, S>,
        cell_checker: &CC,
        header_checker: &HC,
        resolve_opts: ResolveOptions,
    ) -> Result<(), OutPointError> {
        let mut checked_cells: HashSet<OutPoint> = HashSet::new();
        let mut check_cell = |out_point: &OutPoint| -> Result<(), OutPointError> {
            if seen_inputs.contains(out_point) {
                return Err(OutPointError::Dead(out_point.clone()));
            }

            if checked_cells.contains(out_point) {
                return Ok(());
            }

            match cell_checker.is_live(out_point) {
                Some(true) => {
                    checked_cells.insert(out_point.clone());
                    Ok(())
                }
                Some(false) => Err(OutPointError::Dead(out_point.clone())),
                None => Err(OutPointError::Unknown(out_point.clone())),
            }
        };

        // // check input
        for cell_meta in &self.resolved_inputs {
            check_cell(&cell_meta.out_point)?;
        }

        let mut resolved_system_deps: HashSet<&OutPoint> = HashSet::new();
        if let Some(system_cell) = SYSTEM_CELL.get() {
            for cell_meta in &self.resolved_dep_groups {
                let cell_dep = CellDep::new_builder()
                    .out_point(cell_meta.out_point.clone())
                    .dep_type(DepType::DepGroup.into())
                    .build();

                let dep_group = system_cell.get(&cell_dep);
                if let Some(ResolvedDep::Group((_, cell_deps))) = dep_group {
                    resolved_system_deps.extend(cell_deps.iter().map(|dep| &dep.out_point));
                } else {
                    check_cell(&cell_meta.out_point)?;
                }
            }

            for cell_meta in &self.resolved_cell_deps {
                let cell_dep = CellDep::new_builder()
                    .out_point(cell_meta.out_point.clone())
                    .dep_type(DepType::Code.into())
                    .build();

                if system_cell.get(&cell_dep).is_none()
                    && !resolved_system_deps.contains(&cell_meta.out_point)
                {
                    check_cell(&cell_meta.out_point)?;
                }
            }
        } else {
            for cell_meta in self
                .resolved_cell_deps
                .iter()
                .chain(self.resolved_dep_groups.iter())
            {
                check_cell(&cell_meta.out_point)?;
            }
        }

        for block_hash in self.transaction.header_deps_iter() {
            let result = header_checker.check_valid(&block_hash);
            if resolve_opts.if_skip_immature_header_deps_check() {
                if let Err(OutPointError::ImmatureHeader(_)) = result {
                    continue;
                }
            }
            result?;
        }

        seen_inputs.extend(self.resolved_inputs.iter().map(|i| &i.out_point).cloned());

        Ok(())
    }
}

/// Trait for check cell status
pub trait CellChecker {
    /// Returns true if the cell is live corresponding to specified out_point.
    fn is_live(&self, out_point: &OutPoint) -> Option<bool>;
}

/// Overlay cell checker wrapper
pub struct OverlayCellChecker<'a, A, B> {
    overlay: &'a A,
    cell_checker: &'a B,
}

impl<'a, A, B> OverlayCellChecker<'a, A, B>
where
    A: CellChecker,
    B: CellChecker,
{
    /// Construct new OverlayCellChecker
    pub fn new(overlay: &'a A, cell_checker: &'a B) -> Self {
        Self {
            overlay,
            cell_checker,
        }
    }
}

impl<'a, A, B> CellChecker for OverlayCellChecker<'a, A, B>
where
    A: CellChecker,
    B: CellChecker,
{
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        self.overlay
            .is_live(out_point)
            .or_else(|| self.cell_checker.is_live(out_point))
    }
}

/// TODO(doc): @quake
pub trait CellProvider {
    /// TODO(doc): @quake
    fn cell(&self, out_point: &OutPoint, eager_load: bool) -> CellStatus;
}

/// TODO(doc): @quake
pub struct OverlayCellProvider<'a, A, B> {
    overlay: &'a A,
    cell_provider: &'a B,
}

impl<'a, A, B> OverlayCellProvider<'a, A, B>
where
    A: CellProvider,
    B: CellProvider,
{
    /// TODO(doc): @quake
    pub fn new(overlay: &'a A, cell_provider: &'a B) -> Self {
        Self {
            overlay,
            cell_provider,
        }
    }
}

impl<'a, A, B> CellProvider for OverlayCellProvider<'a, A, B>
where
    A: CellProvider,
    B: CellProvider,
{
    fn cell(&self, out_point: &OutPoint, eager_load: bool) -> CellStatus {
        match self.overlay.cell(out_point, eager_load) {
            CellStatus::Live(cell_meta) => CellStatus::Live(cell_meta),
            CellStatus::Dead => CellStatus::Dead,
            CellStatus::Unknown => self.cell_provider.cell(out_point, eager_load),
        }
    }
}

/// TODO(doc): @quake
pub struct BlockCellProvider<'a> {
    output_indices: HashMap<Byte32, usize>,
    block: &'a BlockView,
}

// Transactions are expected to be sorted within a block,
// Transactions have to appear after any transactions upon which they depend
impl<'a> BlockCellProvider<'a> {
    /// TODO(doc): @quake
    pub fn new(block: &'a BlockView) -> Result<Self, Error> {
        let output_indices: HashMap<Byte32, usize> = block
            .transactions()
            .iter()
            .enumerate()
            .map(|(idx, tx)| (tx.hash(), idx))
            .collect();

        for (idx, tx) in block.transactions().iter().enumerate() {
            for dep in tx.cell_deps_iter() {
                if let Some(output_idx) = output_indices.get(&dep.out_point().tx_hash()) {
                    if *output_idx >= idx {
                        return Err(OutPointError::OutOfOrder(dep.out_point()).into());
                    }
                }
            }
            for out_point in tx.input_pts_iter() {
                if let Some(output_idx) = output_indices.get(&out_point.tx_hash()) {
                    if *output_idx >= idx {
                        return Err(OutPointError::OutOfOrder(out_point).into());
                    }
                }
            }
        }

        Ok(Self {
            output_indices,
            block,
        })
    }
}

impl<'a> CellProvider for BlockCellProvider<'a> {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        self.output_indices
            .get(&out_point.tx_hash())
            .and_then(|i| {
                let transaction = self.block.transaction(*i).should_be_ok();
                let j: usize = out_point.index().unpack();
                self.block.output(*i, j).map(|output| {
                    let data = transaction
                        .outputs_data()
                        .get(j)
                        .expect("must exists")
                        .raw_data();
                    let data_hash = CellOutput::calc_data_hash(&data);
                    let header = self.block.header();
                    CellStatus::live_cell(CellMeta {
                        cell_output: output,
                        out_point: out_point.clone(),
                        transaction_info: Some(TransactionInfo {
                            block_number: header.number(),
                            block_epoch: header.epoch(),
                            block_hash: self.block.hash(),
                            index: *i,
                        }),
                        data_bytes: data.len() as u64,
                        mem_cell_data: Some(data),
                        mem_cell_data_hash: Some(data_hash), // make sure load_cell_data_hash works within block
                    })
                })
            })
            .unwrap_or(CellStatus::Unknown)
    }
}

/// Cell checker for txs chain
#[derive(Default)]
pub struct TransactionsChecker {
    inner: HashMap<Byte32, CellOutputVec>,
}

impl TransactionsChecker {
    /// Construct new TransactionsChecker
    pub fn new<'a>(txs: impl Iterator<Item = &'a TransactionView>) -> Self {
        let inner = txs.map(|tx| (tx.hash(), tx.outputs())).collect();
        Self { inner }
    }

    /// append new transaction
    pub fn insert(&mut self, tx: &TransactionView) {
        self.inner.insert(tx.hash(), tx.outputs());
    }
}

impl CellChecker for TransactionsChecker {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        self.inner
            .get(&out_point.tx_hash())
            .and_then(|outputs| outputs.get(out_point.index().unpack()).map(|_| true))
    }
}

/// TODO(doc): @quake
#[derive(Default)]
pub struct TransactionsProvider<'a> {
    transactions: HashMap<Byte32, &'a TransactionView>,
}

impl<'a> TransactionsProvider<'a> {
    /// TODO(doc): @quake
    pub fn new(transactions: impl Iterator<Item = &'a TransactionView>) -> Self {
        let transactions = transactions.map(|tx| (tx.hash(), tx)).collect();
        Self { transactions }
    }

    /// TODO(doc): @quake
    pub fn insert(&mut self, transaction: &'a TransactionView) {
        self.transactions.insert(transaction.hash(), transaction);
    }
}

impl<'a> CellProvider for TransactionsProvider<'a> {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        match self.transactions.get(&out_point.tx_hash()) {
            Some(tx) => tx
                .outputs()
                .get(out_point.index().unpack())
                .map(|cell| {
                    let data = tx
                        .outputs_data()
                        .get(out_point.index().unpack())
                        .expect("output data")
                        .raw_data();
                    let cell_meta = CellMetaBuilder::from_cell_output(cell, data)
                        .out_point(out_point.to_owned())
                        .build();
                    CellStatus::live_cell(cell_meta)
                })
                .unwrap_or(CellStatus::Unknown),
            None => CellStatus::Unknown,
        }
    }
}

/// TODO(doc): @quake
pub trait HeaderChecker {
    /// Check if header in main chain
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), OutPointError>;
}

bitflags! {
    /// Bit flags to set options for resolving transactions.
    pub struct ResolveOptions: u8 {
        /// Skip the immature header deps check.
        const SKIP_IMMATURE_HEADER_DEPS_CHECK               = 0b00000001;
    }
}

impl ResolveOptions {
    /// Get the flag whether skips the immature header deps check or not.
    pub fn if_skip_immature_header_deps_check(self) -> bool {
        self.contains(Self::SKIP_IMMATURE_HEADER_DEPS_CHECK)
    }

    /// Set the flag whether skips the immature header deps check or not.
    pub fn set_skip_immature_header_deps_check(mut self, value: bool) -> Self {
        self.set(Self::SKIP_IMMATURE_HEADER_DEPS_CHECK, value);
        self
    }
}

/// Gather all cell dep out points and resolved dep group out points
pub fn get_related_dep_out_points<F: Fn(&OutPoint) -> Option<Bytes>>(
    tx: &TransactionView,
    get_cell_data: F,
) -> Result<Vec<OutPoint>, String> {
    tx.cell_deps_iter().try_fold(
        Vec::with_capacity(tx.cell_deps().len()),
        |mut out_points, dep| {
            let out_point = dep.out_point();
            if dep.dep_type() == DepType::DepGroup.into() {
                let data = get_cell_data(&out_point)
                    .ok_or_else(|| String::from("Can not get cell data"))?;
                let sub_out_points =
                    parse_dep_group_data(&data).map_err(|err| format!("Invalid data: {}", err))?;
                out_points.extend(sub_out_points.into_iter());
            }
            out_points.push(out_point);
            Ok(out_points)
        },
    )
}

fn parse_dep_group_data(slice: &[u8]) -> Result<OutPointVec, String> {
    if slice.is_empty() {
        Err("data is empty".to_owned())
    } else {
        match OutPointVec::from_slice(slice) {
            Ok(v) => {
                if v.is_empty() {
                    Err("dep group is empty".to_owned())
                } else {
                    Ok(v)
                }
            }
            Err(err) => Err(err.to_string()),
        }
    }
}

fn resolve_dep_group<F: FnMut(&OutPoint, bool) -> Result<CellMeta, OutPointError>>(
    out_point: &OutPoint,
    mut cell_resolver: F,
    eager_load: bool,
) -> Result<(CellMeta, Vec<CellMeta>), OutPointError> {
    let dep_group_cell = cell_resolver(out_point, true)?;
    let data = dep_group_cell
        .mem_cell_data
        .clone()
        .expect("Load cell meta must with data");

    let sub_out_points = parse_dep_group_data(&data)
        .map_err(|_| OutPointError::InvalidDepGroup(out_point.clone()))?;
    let mut resolved_deps = Vec::with_capacity(sub_out_points.len());
    for sub_out_point in sub_out_points.into_iter() {
        resolved_deps.push(cell_resolver(&sub_out_point, eager_load)?);
    }
    Ok((dep_group_cell, resolved_deps))
}

/// **Deprecated** Use `resolve_transaction_with_options` instead of.
///
/// This function is only used to be compatible with the old unit tests.
/// Please don't add more tests with this function.
/// Then we could remove this function easier in future.
pub fn resolve_transaction<CP: CellProvider, HC: HeaderChecker, S: BuildHasher>(
    transaction: TransactionView,
    seen_inputs: &mut HashSet<OutPoint, S>,
    cell_provider: &CP,
    header_checker: &HC,
) -> Result<ResolvedTransaction, OutPointError> {
    resolve_transaction_with_options(
        transaction,
        seen_inputs,
        cell_provider,
        header_checker,
        ResolveOptions::empty().set_skip_immature_header_deps_check(false),
    )
}

/// TODO(doc): @quake
pub fn resolve_transaction_with_options<CP: CellProvider, HC: HeaderChecker, S: BuildHasher>(
    transaction: TransactionView,
    seen_inputs: &mut HashSet<OutPoint, S>,
    cell_provider: &CP,
    header_checker: &HC,
    resolve_opts: ResolveOptions,
) -> Result<ResolvedTransaction, OutPointError> {
    let (mut resolved_inputs, mut resolved_cell_deps, mut resolved_dep_groups) = (
        Vec::with_capacity(transaction.inputs().len()),
        Vec::with_capacity(transaction.cell_deps().len()),
        Vec::new(),
    );
    let mut current_inputs = HashSet::new();

    let mut resolved_cells: HashMap<(OutPoint, bool), CellMeta> = HashMap::new();
    let mut resolve_cell =
        |out_point: &OutPoint, eager_load: bool| -> Result<CellMeta, OutPointError> {
            if seen_inputs.contains(out_point) {
                return Err(OutPointError::Dead(out_point.clone()));
            }

            match resolved_cells.entry((out_point.clone(), eager_load)) {
                Entry::Occupied(entry) => Ok(entry.get().clone()),
                Entry::Vacant(entry) => {
                    let cell_status = cell_provider.cell(out_point, eager_load);
                    match cell_status {
                        CellStatus::Dead => Err(OutPointError::Dead(out_point.clone())),
                        CellStatus::Unknown => Err(OutPointError::Unknown(out_point.clone())),
                        CellStatus::Live(cell_meta) => {
                            entry.insert(cell_meta.clone());
                            Ok(cell_meta)
                        }
                    }
                }
            }
        };

    // skip resolve input of cellbase
    if !transaction.is_cellbase() {
        for out_point in transaction.input_pts_iter() {
            if !current_inputs.insert(out_point.to_owned()) {
                return Err(OutPointError::Dead(out_point));
            }
            resolved_inputs.push(resolve_cell(&out_point, false)?);
        }
    }

    resolve_transaction_deps_with_system_cell_cache(
        &transaction,
        &mut resolve_cell,
        &mut resolved_cell_deps,
        &mut resolved_dep_groups,
    )?;

    for block_hash in transaction.header_deps_iter() {
        let result = header_checker.check_valid(&block_hash);
        if resolve_opts.if_skip_immature_header_deps_check() {
            if let Err(OutPointError::ImmatureHeader(_)) = result {
                continue;
            }
        }
        result?;
    }

    seen_inputs.extend(current_inputs);
    Ok(ResolvedTransaction {
        transaction,
        resolved_inputs,
        resolved_cell_deps,
        resolved_dep_groups,
    })
}

fn resolve_transaction_deps_with_system_cell_cache<
    F: FnMut(&OutPoint, bool) -> Result<CellMeta, OutPointError>,
>(
    transaction: &TransactionView,
    cell_resolver: &mut F,
    resolved_cell_deps: &mut Vec<CellMeta>,
    resolved_dep_groups: &mut Vec<CellMeta>,
) -> Result<(), OutPointError> {
    if let Some(system_cell) = SYSTEM_CELL.get() {
        for cell_dep in transaction.cell_deps_iter() {
            if let Some(resolved_dep) = system_cell.get(&cell_dep) {
                match resolved_dep {
                    ResolvedDep::Cell(cell_meta) => resolved_cell_deps.push(cell_meta.clone()),
                    ResolvedDep::Group(group) => {
                        let (dep_group, cell_deps) = group;
                        resolved_dep_groups.push(dep_group.clone());
                        resolved_cell_deps.extend(cell_deps.clone());
                    }
                }
            } else {
                resolve_transaction_dep(
                    &cell_dep,
                    cell_resolver,
                    resolved_cell_deps,
                    resolved_dep_groups,
                    false, // don't eager_load data
                )?;
            }
        }
    } else {
        for cell_dep in transaction.cell_deps_iter() {
            resolve_transaction_dep(
                &cell_dep,
                cell_resolver,
                resolved_cell_deps,
                resolved_dep_groups,
                false, // don't eager_load data
            )?;
        }
    }
    Ok(())
}

fn resolve_transaction_dep<F: FnMut(&OutPoint, bool) -> Result<CellMeta, OutPointError>>(
    cell_dep: &CellDep,
    cell_resolver: &mut F,
    resolved_cell_deps: &mut Vec<CellMeta>,
    resolved_dep_groups: &mut Vec<CellMeta>,
    eager_load: bool,
) -> Result<(), OutPointError> {
    if cell_dep.dep_type() == DepType::DepGroup.into() {
        let (dep_group, cell_deps) =
            resolve_dep_group(&cell_dep.out_point(), cell_resolver, eager_load)?;
        resolved_dep_groups.push(dep_group);
        resolved_cell_deps.extend(cell_deps);
    } else {
        resolved_cell_deps.push(cell_resolver(&cell_dep.out_point(), eager_load)?);
    }
    Ok(())
}

fn build_cell_meta_from_out_point<CP: CellProvider>(
    cell_provider: &CP,
    out_point: &OutPoint,
) -> Result<CellMeta, OutPointError> {
    // eager_load data for build cache
    let cell_status = cell_provider.cell(out_point, true);
    match cell_status {
        CellStatus::Dead => Err(OutPointError::Dead(out_point.clone())),
        CellStatus::Unknown => Err(OutPointError::Unknown(out_point.clone())),
        CellStatus::Live(cell_meta) => Ok(cell_meta),
    }
}

/// TODO(doc): @quake
pub fn setup_system_cell_cache<CP: CellProvider>(
    genesis: &BlockView,
    cell_provider: &CP,
) -> Result<(), SystemCellMap> {
    let system_cell_transaction = &genesis.transactions()[0];
    let secp_cell_transaction = &genesis.transactions()[1];
    let secp_code_dep = CellDep::new_builder()
        .out_point(OutPoint::new(system_cell_transaction.hash(), 1))
        .dep_type(DepType::Code.into())
        .build();

    let dao_dep = CellDep::new_builder()
        .out_point(OutPoint::new(system_cell_transaction.hash(), 2))
        .dep_type(DepType::Code.into())
        .build();

    let secp_data_dep = CellDep::new_builder()
        .out_point(OutPoint::new(system_cell_transaction.hash(), 3))
        .dep_type(DepType::Code.into())
        .build();

    let secp_group_dep = CellDep::new_builder()
        .out_point(OutPoint::new(secp_cell_transaction.hash(), 0))
        .dep_type(DepType::DepGroup.into())
        .build();

    let multi_sign_secp_group = CellDep::new_builder()
        .out_point(OutPoint::new(secp_cell_transaction.hash(), 1))
        .dep_type(DepType::DepGroup.into())
        .build();

    let mut cell_deps = HashMap::new();
    let secp_code_dep_cell =
        build_cell_meta_from_out_point(cell_provider, &secp_code_dep.out_point())
            .expect("resolve secp_code_dep_cell");
    cell_deps.insert(secp_code_dep, ResolvedDep::Cell(secp_code_dep_cell));

    let dao_dep_cell = build_cell_meta_from_out_point(cell_provider, &dao_dep.out_point())
        .expect("resolve dao_dep_cell");
    cell_deps.insert(dao_dep, ResolvedDep::Cell(dao_dep_cell));

    let secp_data_dep_cell =
        build_cell_meta_from_out_point(cell_provider, &secp_data_dep.out_point())
            .expect("resolve secp_data_dep_cell");
    cell_deps.insert(secp_data_dep, ResolvedDep::Cell(secp_data_dep_cell));

    // eager_load data for build cache
    let resolve_cell =
        |out_point: &OutPoint, _eager_load: bool| -> Result<CellMeta, OutPointError> {
            build_cell_meta_from_out_point(cell_provider, out_point)
        };

    let secp_group_dep_cell = resolve_dep_group(&secp_group_dep.out_point(), resolve_cell, true)
        .expect("resolve secp_group_dep_cell");
    cell_deps.insert(secp_group_dep, ResolvedDep::Group(secp_group_dep_cell));

    let multi_sign_secp_group_cell =
        resolve_dep_group(&multi_sign_secp_group.out_point(), resolve_cell, true)
            .expect("resolve multi_sign_secp_group");
    cell_deps.insert(
        multi_sign_secp_group,
        ResolvedDep::Group(multi_sign_secp_group_cell),
    );

    SYSTEM_CELL.set(cell_deps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{
            capacity_bytes, BlockBuilder, BlockView, Capacity, EpochNumberWithFraction,
            TransactionBuilder,
        },
        h256,
        packed::{Byte32, CellDep, CellInput},
        H256,
    };
    use ckb_error::assert_error_eq;
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct BlockHeadersChecker {
        attached_indices: HashSet<Byte32>,
        detached_indices: HashSet<Byte32>,
    }

    impl BlockHeadersChecker {
        pub fn push_attached(&mut self, block_hash: Byte32) {
            self.attached_indices.insert(block_hash);
        }
    }

    impl HeaderChecker for BlockHeadersChecker {
        fn check_valid(&self, block_hash: &Byte32) -> Result<(), OutPointError> {
            if !self.detached_indices.contains(block_hash)
                && self.attached_indices.contains(block_hash)
            {
                Ok(())
            } else {
                Err(OutPointError::InvalidHeader(block_hash.clone()))
            }
        }
    }

    #[derive(Default)]
    struct CellMemoryDb {
        cells: HashMap<OutPoint, Option<CellMeta>>,
    }
    impl CellProvider for CellMemoryDb {
        fn cell(&self, o: &OutPoint, _eager_load: bool) -> CellStatus {
            match self.cells.get(o) {
                Some(&Some(ref cell_meta)) => CellStatus::live_cell(cell_meta.clone()),
                Some(&None) => CellStatus::Dead,
                None => CellStatus::Unknown,
            }
        }
    }

    fn generate_dummy_cell_meta_with_info(out_point: OutPoint, data: Bytes) -> CellMeta {
        let cell_output = CellOutput::new_builder()
            .capacity(capacity_bytes!(2).pack())
            .build();
        let data_hash = CellOutput::calc_data_hash(&data);
        CellMeta {
            transaction_info: Some(TransactionInfo {
                block_number: 1,
                block_epoch: EpochNumberWithFraction::new(1, 1, 10),
                block_hash: Byte32::zero(),
                index: 1,
            }),
            cell_output,
            out_point,
            data_bytes: data.len() as u64,
            mem_cell_data: Some(data),
            mem_cell_data_hash: Some(data_hash),
        }
    }

    fn generate_dummy_cell_meta_with_out_point(out_point: OutPoint) -> CellMeta {
        generate_dummy_cell_meta_with_info(out_point, Bytes::default())
    }

    fn generate_dummy_cell_meta_with_data(data: Bytes) -> CellMeta {
        generate_dummy_cell_meta_with_info(OutPoint::new(Default::default(), 0), data)
    }

    fn generate_dummy_cell_meta() -> CellMeta {
        generate_dummy_cell_meta_with_data(Bytes::default())
    }

    fn generate_block(txs: Vec<TransactionView>) -> BlockView {
        BlockBuilder::default().transactions(txs).build()
    }

    #[test]
    fn cell_provider_trait_works() {
        let mut db = CellMemoryDb::default();

        let p1 = OutPoint::new(Byte32::zero(), 1);
        let p2 = OutPoint::new(Byte32::zero(), 2);
        let p3 = OutPoint::new(Byte32::zero(), 3);
        let o = generate_dummy_cell_meta();

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(CellStatus::Live(o), db.cell(&p1, false));
        assert_eq!(CellStatus::Dead, db.cell(&p2, false));
        assert_eq!(CellStatus::Unknown, db.cell(&p3, false));
    }

    #[test]
    fn resolve_transaction_should_resolve_dep_group() {
        let mut cell_provider = CellMemoryDb::default();
        let header_checker = BlockHeadersChecker::default();

        let op_dep = OutPoint::new(Byte32::zero(), 72);
        let op_1 = OutPoint::new(h256!("0x13").pack(), 1);
        let op_2 = OutPoint::new(h256!("0x23").pack(), 2);
        let op_3 = OutPoint::new(h256!("0x33").pack(), 3);

        for op in &[&op_1, &op_2, &op_3] {
            cell_provider.cells.insert(
                (*op).clone(),
                Some(generate_dummy_cell_meta_with_out_point((*op).clone())),
            );
        }
        let cell_data = vec![op_1.clone(), op_2.clone(), op_3.clone()]
            .pack()
            .as_bytes();
        let dep_group_cell = generate_dummy_cell_meta_with_data(cell_data);
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::new_builder()
            .out_point(op_dep)
            .dep_type(DepType::DepGroup.into())
            .build();

        let transaction = TransactionBuilder::default().cell_dep(dep).build();
        let mut seen_inputs = HashSet::new();
        let result = resolve_transaction(
            transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        )
        .unwrap();

        assert_eq!(result.resolved_cell_deps.len(), 3);
        assert_eq!(result.resolved_cell_deps[0].out_point, op_1);
        assert_eq!(result.resolved_cell_deps[1].out_point, op_2);
        assert_eq!(result.resolved_cell_deps[2].out_point, op_3);
    }

    #[test]
    fn resolve_transaction_resolve_dep_group_failed_because_invalid_data() {
        let mut cell_provider = CellMemoryDb::default();
        let header_checker = BlockHeadersChecker::default();

        let op_dep = OutPoint::new(Byte32::zero(), 72);
        let cell_data = Bytes::from("this is invalid data");
        let dep_group_cell = generate_dummy_cell_meta_with_data(cell_data);
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::new_builder()
            .out_point(op_dep.clone())
            .dep_type(DepType::DepGroup.into())
            .build();

        let transaction = TransactionBuilder::default().cell_dep(dep).build();
        let mut seen_inputs = HashSet::new();
        let result = resolve_transaction(
            transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        );
        assert_error_eq!(result.unwrap_err(), OutPointError::InvalidDepGroup(op_dep));
    }

    #[test]
    fn resolve_transaction_resolve_dep_group_failed_because_unknown_sub_cell() {
        let mut cell_provider = CellMemoryDb::default();
        let header_checker = BlockHeadersChecker::default();

        let op_unknown = OutPoint::new(h256!("0x45").pack(), 5);
        let op_dep = OutPoint::new(Byte32::zero(), 72);
        let cell_data = vec![op_unknown.clone()].pack().as_bytes();
        let dep_group_cell = generate_dummy_cell_meta_with_data(cell_data);
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::new_builder()
            .out_point(op_dep)
            .dep_type(DepType::DepGroup.into())
            .build();

        let transaction = TransactionBuilder::default().cell_dep(dep).build();
        let mut seen_inputs = HashSet::new();
        let result = resolve_transaction(
            transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        );
        assert_error_eq!(result.unwrap_err(), OutPointError::Unknown(op_unknown),);
    }

    #[test]
    fn resolve_transaction_test_header_deps_all_ok() {
        let cell_provider = CellMemoryDb::default();
        let mut header_checker = BlockHeadersChecker::default();

        let block_hash1 = h256!("0x1111").pack();
        let block_hash2 = h256!("0x2222").pack();

        header_checker.push_attached(block_hash1.clone());
        header_checker.push_attached(block_hash2.clone());

        let transaction = TransactionBuilder::default()
            .header_dep(block_hash1)
            .header_dep(block_hash2)
            .build();

        let mut seen_inputs = HashSet::new();
        let result = resolve_transaction(
            transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn resolve_transaction_should_test_have_invalid_header_dep() {
        let cell_provider = CellMemoryDb::default();
        let mut header_checker = BlockHeadersChecker::default();

        let main_chain_block_hash = h256!("0xaabbcc").pack();
        let invalid_block_hash = h256!("0x3344").pack();

        header_checker.push_attached(main_chain_block_hash.clone());

        let transaction = TransactionBuilder::default()
            .header_dep(main_chain_block_hash)
            .header_dep(invalid_block_hash.clone())
            .build();

        let mut seen_inputs = HashSet::new();
        let result = resolve_transaction(
            transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        );

        assert_error_eq!(
            result.unwrap_err(),
            OutPointError::InvalidHeader(invalid_block_hash),
        );
    }

    #[test]
    fn resolve_transaction_should_reject_incorrect_order_txs() {
        let out_point = OutPoint::new(h256!("0x2").pack(), 3);

        let tx1 = TransactionBuilder::default()
            .input(CellInput::new(out_point, 0))
            .output(
                CellOutput::new_builder()
                    .capacity(capacity_bytes!(2).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx2 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx1.hash(), 0), 0))
            .build();

        let dep = CellDep::new_builder()
            .out_point(OutPoint::new(tx1.hash(), 0))
            .build();
        let tx3 = TransactionBuilder::default().cell_dep(dep).build();

        // tx1 <- tx2
        // ok
        {
            let block = generate_block(vec![tx1.clone(), tx2.clone()]);
            let provider = BlockCellProvider::new(&block);
            assert!(provider.is_ok());
        }

        // tx1 -> tx2
        // resolve err
        {
            let block = generate_block(vec![tx2, tx1.clone()]);
            let provider = BlockCellProvider::new(&block);

            assert_error_eq!(
                provider.err().unwrap(),
                OutPointError::OutOfOrder(OutPoint::new(tx1.hash(), 0)),
            );
        }

        // tx1 <- tx3
        // ok
        {
            let block = generate_block(vec![tx1.clone(), tx3.clone()]);
            let provider = BlockCellProvider::new(&block);

            assert!(provider.is_ok());
        }

        // tx1 -> tx3
        // resolve err
        {
            let block = generate_block(vec![tx3, tx1.clone()]);
            let provider = BlockCellProvider::new(&block);

            assert_error_eq!(
                provider.err().unwrap(),
                OutPointError::OutOfOrder(OutPoint::new(tx1.hash(), 0)),
            );
        }
    }

    #[test]
    fn resolve_transaction_should_allow_dep_cell_in_current_tx_input() {
        let mut cell_provider = CellMemoryDb::default();
        let header_checker = BlockHeadersChecker::default();

        let out_point = OutPoint::new(h256!("0x2").pack(), 3);

        let dummy_cell_meta = generate_dummy_cell_meta();
        cell_provider
            .cells
            .insert(out_point.clone(), Some(dummy_cell_meta.clone()));

        let dep = CellDep::new_builder().out_point(out_point.clone()).build();
        let tx = TransactionBuilder::default()
            .input(CellInput::new(out_point, 0))
            .cell_dep(dep)
            .build();

        let mut seen_inputs = HashSet::new();
        let rtx =
            resolve_transaction(tx, &mut seen_inputs, &cell_provider, &header_checker).unwrap();

        assert_eq!(rtx.resolved_cell_deps[0], dummy_cell_meta,);
    }

    #[test]
    fn resolve_transaction_should_reject_dep_cell_consumed_by_previous_input() {
        let mut cell_provider = CellMemoryDb::default();
        let header_checker = BlockHeadersChecker::default();

        let out_point = OutPoint::new(h256!("0x2").pack(), 3);

        cell_provider
            .cells
            .insert(out_point.clone(), Some(generate_dummy_cell_meta()));

        // tx1 dep
        // tx2 input consumed
        // ok
        {
            let dep = CellDep::new_builder().out_point(out_point.clone()).build();
            let tx1 = TransactionBuilder::default().cell_dep(dep).build();
            let tx2 = TransactionBuilder::default()
                .input(CellInput::new(out_point.clone(), 0))
                .build();

            let mut seen_inputs = HashSet::new();
            let result1 =
                resolve_transaction(tx1, &mut seen_inputs, &cell_provider, &header_checker);
            assert!(result1.is_ok());

            let result2 =
                resolve_transaction(tx2, &mut seen_inputs, &cell_provider, &header_checker);
            assert!(result2.is_ok());
        }

        // tx1 input consumed
        // tx2 dep
        // tx2 resolve err
        {
            let tx1 = TransactionBuilder::default()
                .input(CellInput::new(out_point.clone(), 0))
                .build();

            let dep = CellDep::new_builder().out_point(out_point.clone()).build();
            let tx2 = TransactionBuilder::default().cell_dep(dep).build();

            let mut seen_inputs = HashSet::new();
            let result1 =
                resolve_transaction(tx1, &mut seen_inputs, &cell_provider, &header_checker);

            assert!(result1.is_ok());

            let result2 =
                resolve_transaction(tx2, &mut seen_inputs, &cell_provider, &header_checker);

            assert_error_eq!(result2.unwrap_err(), OutPointError::Dead(out_point));
        }
    }
}
