use crate::block::Block;
use crate::header::Header;
use crate::transaction::{CellDep, CellOutput, OutPoint, Transaction};
use crate::{BlockNumber, EpochNumber};
use crate::{Bytes, Capacity};
use ckb_occupied_capacity::Result as CapacityResult;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::convert::TryInto;
use std::fmt;
use std::mem;

#[derive(Clone, Debug, Eq, PartialEq, Default, Deserialize, Serialize)]
pub struct BlockInfo {
    pub number: BlockNumber,
    pub epoch: EpochNumber,
    pub hash: H256,
}

impl BlockInfo {
    pub fn new(number: BlockNumber, epoch: EpochNumber, hash: H256) -> Self {
        BlockInfo {
            number,
            epoch,
            hash,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Default, Deserialize, Serialize)]
pub struct CellMeta {
    #[serde(skip)]
    pub cell_output: CellOutput,
    pub out_point: OutPoint,
    pub block_info: Option<BlockInfo>,
    pub cellbase: bool,
    pub data_bytes: u64,
    /// In memory cell data
    /// A live cell either exists in memory or DB
    /// must check DB if this field is None
    #[serde(skip)]
    pub mem_cell_data: Option<Bytes>,
}

#[derive(Default)]
pub struct CellMetaBuilder {
    cell_output: CellOutput,
    out_point: OutPoint,
    block_info: Option<BlockInfo>,
    cellbase: bool,
    data_bytes: u64,
    mem_cell_data: Option<Bytes>,
}

impl CellMetaBuilder {
    pub fn from_cell_meta(cell_meta: CellMeta) -> Self {
        let CellMeta {
            cell_output,
            out_point,
            block_info,
            cellbase,
            data_bytes,
            mem_cell_data,
        } = cell_meta;
        Self {
            cell_output,
            out_point,
            block_info,
            cellbase,
            data_bytes,
            mem_cell_data,
        }
    }

    pub fn from_cell_output(cell_output: CellOutput, data: Bytes) -> Self {
        let mut builder = CellMetaBuilder::default();
        builder.cell_output = cell_output;
        builder.data_bytes = data.len().try_into().expect("u32");
        builder.mem_cell_data = Some(data);
        builder
    }

    pub fn out_point(mut self, out_point: OutPoint) -> Self {
        self.out_point = out_point;
        self
    }

    pub fn block_info(mut self, block_info: BlockInfo) -> Self {
        self.block_info = Some(block_info);
        self
    }

    pub fn cellbase(mut self, cellbase: bool) -> Self {
        self.cellbase = cellbase;
        self
    }

    pub fn build(self) -> CellMeta {
        let Self {
            cell_output,
            out_point,
            block_info,
            cellbase,
            data_bytes,
            mem_cell_data,
        } = self;
        CellMeta {
            cell_output,
            out_point,
            block_info,
            cellbase,
            data_bytes,
            mem_cell_data,
        }
    }
}

impl fmt::Debug for CellMeta {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CellMeta")
            .field("cell_output", &self.cell_output)
            .field("out_point", &self.out_point)
            .field("block_info", &self.block_info)
            .field("cellbase", &self.cellbase)
            .field("data_bytes", &self.data_bytes)
            .finish()
    }
}

impl CellMeta {
    pub fn is_cellbase(&self) -> bool {
        self.cellbase
    }

    pub fn capacity(&self) -> Capacity {
        self.cell_output.capacity
    }

    pub fn data_hash(&self) -> &H256 {
        &self.cell_output.data_hash
    }

    pub fn occupied_capacity(&self) -> CapacityResult<Capacity> {
        self.cell_output
            .occupied_capacity(Capacity::bytes(self.data_bytes as usize)?)
    }

    pub fn is_lack_of_capacity(&self) -> CapacityResult<bool> {
        self.cell_output
            .is_lack_of_capacity(Capacity::bytes(self.data_bytes as usize)?)
    }
}

#[derive(PartialEq, Debug)]
pub enum CellStatus {
    /// Cell exists and has not been spent.
    Live(Box<CellMeta>),
    /// Cell exists and has been spent.
    Dead,
    /// Cell does not exist.
    Unknown,
}

impl CellStatus {
    pub fn live_cell(cell_meta: CellMeta) -> CellStatus {
        CellStatus::Live(Box::new(cell_meta))
    }

    pub fn is_live(&self) -> bool {
        match *self {
            CellStatus::Live(_) => true,
            _ => false,
        }
    }

    pub fn is_dead(&self) -> bool {
        self == &CellStatus::Dead
    }

    pub fn is_unknown(&self) -> bool {
        self == &CellStatus::Unknown
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum HeaderStatus {
    /// Header exists on current chain
    Live(Box<Header>),
    /// Header exists, but the specified block doesn't contain referenced transaction.
    InclusionFaliure,
    /// Header does not exist on current chain
    Unknown,
}

impl HeaderStatus {
    pub fn live_header(header: Header) -> HeaderStatus {
        HeaderStatus::Live(Box::new(header))
    }

    pub fn is_live(&self) -> bool {
        match *self {
            HeaderStatus::Live(_) => true,
            _ => false,
        }
    }

    pub fn is_inclusion_failure(&self) -> bool {
        self == &HeaderStatus::InclusionFaliure
    }

    pub fn is_unknown(&self) -> bool {
        self == &HeaderStatus::Unknown
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInput {
    cell: CellMeta,
}

impl ResolvedInput {
    pub fn new(cell: CellMeta) -> ResolvedInput {
        ResolvedInput { cell }
    }

    pub fn cell(&self) -> &CellMeta {
        &self.cell
    }

    pub fn into_inner(self) -> CellMeta {
        self.cell
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDep {
    cell: Option<CellMeta>,
    header: Option<Header>,
}

impl ResolvedDep {
    pub fn cell_only(cell: CellMeta) -> ResolvedDep {
        ResolvedDep {
            cell: Some(cell),
            header: None,
        }
    }

    pub fn cell_and_header(cell: CellMeta, header: Header) -> ResolvedDep {
        ResolvedDep {
            cell: Some(cell),
            header: Some(header),
        }
    }

    pub fn header_only(header: Header) -> ResolvedDep {
        ResolvedDep {
            cell: None,
            header: Some(header),
        }
    }

    pub fn cell(&self) -> Option<&CellMeta> {
        self.cell.as_ref()
    }

    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    pub fn destruct(self) -> (Option<CellMeta>, Option<Header>) {
        (self.cell, self.header)
    }
}

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction<'a> {
    pub transaction: &'a Transaction,
    pub resolved_deps: Vec<ResolvedDep>,
    pub resolved_inputs: Vec<ResolvedInput>,
}

pub trait CellProvider {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus;
}

pub struct OverlayCellProvider<'a, A, B> {
    overlay: &'a A,
    cell_provider: &'a B,
}

impl<'a, A, B> OverlayCellProvider<'a, A, B>
where
    A: CellProvider,
    B: CellProvider,
{
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
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        match self.overlay.cell(out_point, with_data) {
            CellStatus::Live(cell_meta) => CellStatus::Live(cell_meta),
            CellStatus::Dead => CellStatus::Dead,
            CellStatus::Unknown => self.cell_provider.cell(out_point, with_data),
        }
    }
}

pub struct BlockCellProvider<'a> {
    output_indices: FnvHashMap<&'a H256, usize>,
    block: &'a Block,
}

// Transactions are expected to be sorted within a block,
// Transactions have to appear after any transactions upon which they depend
impl<'a> BlockCellProvider<'a> {
    pub fn new(block: &'a Block) -> Result<Self, UnresolvableError> {
        let output_indices: FnvHashMap<&'a H256, usize> = block
            .transactions()
            .iter()
            .enumerate()
            .map(|(idx, tx)| (tx.hash(), idx))
            .collect();

        for (idx, tx) in block.transactions().iter().enumerate() {
            for dep in tx.deps_iter() {
                let out_point_opt = match dep {
                    CellDep::Cell(out_point) => Some(out_point),
                    CellDep::CellWithHeader(out_point, _) => Some(out_point),
                    CellDep::DepGroup(out_point) => Some(out_point),
                    CellDep::Header(_) => None,
                };
                if let Some(out_point) = out_point_opt {
                    if let Some(output_idx) = output_indices.get(&out_point.tx_hash) {
                        if *output_idx >= idx {
                            return Err(UnresolvableError::OutOfOrder(out_point.clone()));
                        }
                    }
                }
            }
            for out_point in tx.input_pts_iter() {
                if let Some(output_idx) = output_indices.get(&out_point.tx_hash) {
                    if *output_idx >= idx {
                        return Err(UnresolvableError::OutOfOrder(out_point.clone()));
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
    fn cell(&self, out_point: &OutPoint, _with_data: bool) -> CellStatus {
        self.output_indices
            .get(&out_point.tx_hash)
            .and_then(|i| {
                let transaction = &self.block.transactions()[*i];
                transaction
                    .outputs()
                    .get(out_point.index as usize)
                    .map(|output| {
                        let data = transaction
                            .outputs_data()
                            .get(out_point.index as usize)
                            .map(ToOwned::to_owned)
                            .expect("must exists");
                        CellStatus::live_cell(CellMeta {
                            cell_output: output.to_owned(),
                            out_point: out_point.to_owned(),
                            block_info: Some(BlockInfo {
                                number: self.block.header().number(),
                                epoch: self.block.header().epoch(),
                                hash: self.block.header().hash().to_owned(),
                            }),
                            cellbase: *i == 0,
                            data_bytes: data.len() as u64,
                            mem_cell_data: Some(data),
                        })
                    })
            })
            .unwrap_or_else(|| CellStatus::Unknown)
    }
}

pub struct TransactionsProvider {
    transactions: FnvHashMap<H256, Transaction>,
}

impl TransactionsProvider {
    pub fn new(transactions: &[Transaction]) -> Self {
        let transactions = transactions
            .iter()
            .map(|tx| (tx.hash().to_owned(), tx.to_owned()))
            .collect();
        Self { transactions }
    }
}

impl CellProvider for TransactionsProvider {
    fn cell(&self, out_point: &OutPoint, _with_data: bool) -> CellStatus {
        match self.transactions.get(&out_point.tx_hash) {
            Some(tx) => tx
                .outputs()
                .get(out_point.index as usize)
                .as_ref()
                .map(|cell| {
                    let data = tx
                        .outputs_data()
                        .get(out_point.index as usize)
                        .expect("output data");
                    CellStatus::live_cell(
                        CellMetaBuilder::from_cell_output((*cell).to_owned(), data.to_owned())
                            .build(),
                    )
                })
                .unwrap_or(CellStatus::Unknown),
            None => CellStatus::Unknown,
        }
    }
}

pub trait HeaderProvider {
    fn header(&self, block_hash: &H256, out_point: Option<&OutPoint>) -> HeaderStatus;
}

#[derive(Default)]
pub struct BlockHeadersProvider {
    attached_indices: FnvHashMap<H256, Header>,
    attached_transaction_blocks: FnvHashMap<H256, H256>,
    detached_indices: FnvHashMap<H256, Header>,
}

impl BlockHeadersProvider {
    pub fn push_attached(&mut self, block: &Block) {
        self.attached_indices
            .insert(block.header().hash().clone(), block.header().clone());
        for tx in block.transactions() {
            self.attached_transaction_blocks
                .insert(tx.hash().clone(), block.header().hash().clone());
        }
    }

    pub fn push_detached(&mut self, block: &Block) {
        self.detached_indices
            .insert(block.header().hash().clone(), block.header().clone());
    }

    #[cfg(test)]
    pub fn insert_attached_transaction_block(&mut self, tx_hash: H256, header_hash: H256) {
        self.attached_transaction_blocks
            .insert(tx_hash, header_hash);
    }
}

impl HeaderProvider for BlockHeadersProvider {
    fn header(&self, block_hash: &H256, out_point: Option<&OutPoint>) -> HeaderStatus {
        if self.detached_indices.contains_key(block_hash) {
            return HeaderStatus::Unknown;
        }
        match self.attached_indices.get(block_hash) {
            Some(header) => {
                if let Some(out_point) = out_point {
                    self.attached_transaction_blocks
                        .get(&out_point.tx_hash)
                        .map_or(HeaderStatus::InclusionFaliure, |tx_block_hash| {
                            if *tx_block_hash == *block_hash {
                                HeaderStatus::live_header((*header).clone())
                            } else {
                                HeaderStatus::InclusionFaliure
                            }
                        })
                } else {
                    HeaderStatus::live_header((*header).clone())
                }
            }
            None => HeaderStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UnresolvableError {
    // OutPoint specifies an invalid header, this could be due to either
    // of the following 2 reasons:
    // 1. Specified header doesn't exist on chain.
    // 2. OutPoint specifies both header and cell, but the specified cell
    // is not included in the specified block header.
    InvalidHeader(H256, Option<OutPoint>),
    InvalidDepGroup(OutPoint),
    Dead(OutPoint),
    Unknown(Vec<OutPoint>),
    OutOfOrder(OutPoint),
}

pub fn parse_dep_group_data(data: Bytes) -> Result<Vec<OutPoint>, usize> {
    // tx hash (32 bytes) + output index (4 bytes)
    const OUT_POINT_LEN: usize = mem::size_of::<H256>() + mem::size_of::<u32>();

    if data.is_empty() || data.len() % OUT_POINT_LEN != 0 {
        return Err(data.len());
    }

    let dep_len = data.len() / OUT_POINT_LEN;
    let mut out_points = Vec::with_capacity(dep_len);
    for dep_idx in 0..dep_len {
        let item_start = dep_idx * OUT_POINT_LEN;
        let item_end = (dep_idx + 1) * OUT_POINT_LEN;
        out_points.push(
            OutPoint::from_group_data(&data[item_start..item_end])
                .expect("parse group item data failed"),
        );
    }
    Ok(out_points)
}

fn resolve_dep_group<
    F: FnMut(&OutPoint, bool) -> Result<Option<Box<CellMeta>>, UnresolvableError>,
>(
    out_point: &OutPoint,
    mut cell_resolver: F,
) -> Result<Vec<ResolvedDep>, UnresolvableError> {
    let data = match cell_resolver(out_point, true)? {
        Some(cell_meta) => cell_meta
            .mem_cell_data
            .expect("Load cell meta must with data"),
        None => return Ok(Vec::new()),
    };

    let sub_out_points = parse_dep_group_data(data)
        .map_err(|_| UnresolvableError::InvalidDepGroup(out_point.clone()))?;
    let mut resolved_deps = Vec::with_capacity(sub_out_points.len());
    for sub_out_point in sub_out_points {
        if let Some(sub_cell_meta) = cell_resolver(&sub_out_point, false)? {
            resolved_deps.push(ResolvedDep::cell_only(*sub_cell_meta));
        }
    }
    Ok(resolved_deps)
}

pub fn resolve_transaction<'a, CP: CellProvider, HP: HeaderProvider>(
    transaction: &'a Transaction,
    seen_inputs: &mut FnvHashSet<OutPoint>,
    cell_provider: &CP,
    header_provider: &HP,
) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
    let (mut unknown_out_points, mut resolved_inputs, mut resolved_deps) = (
        Vec::new(),
        Vec::with_capacity(transaction.inputs().len()),
        Vec::with_capacity(transaction.deps().len()),
    );
    let mut current_inputs = FnvHashSet::default();

    let mut resolve_cell = |out_point: &OutPoint,
                            with_data: bool|
     -> Result<Option<Box<CellMeta>>, UnresolvableError> {
        if seen_inputs.contains(out_point) {
            return Err(UnresolvableError::Dead(out_point.clone()));
        }

        let cell_status = cell_provider.cell(out_point, with_data);
        match cell_status {
            CellStatus::Dead => Err(UnresolvableError::Dead(out_point.clone())),
            CellStatus::Unknown => {
                unknown_out_points.push(out_point.clone());
                Ok(None)
            }
            CellStatus::Live(cell_meta) => Ok(Some(cell_meta)),
        }
    };

    // skip resolve input of cellbase
    if !transaction.is_cellbase() {
        for out_point in transaction.input_pts_iter() {
            if !current_inputs.insert(out_point.to_owned()) {
                return Err(UnresolvableError::Dead(out_point.to_owned()));
            }
            if let Some(cell_meta) = resolve_cell(out_point, false)? {
                resolved_inputs.push(ResolvedInput::new(*cell_meta));
            }
        }
    }

    let resolve_header = |block_hash: &H256,
                          out_point: Option<&OutPoint>|
     -> Result<Box<Header>, UnresolvableError> {
        match header_provider.header(block_hash, out_point) {
            HeaderStatus::Unknown => {
                // TODO: should we change transaction pool so transactions
                // with unknown header can be included as orphans, waiting
                // for the correct block header to enable it?
                Err(UnresolvableError::InvalidHeader(
                    block_hash.clone(),
                    out_point.cloned(),
                ))
            }
            HeaderStatus::InclusionFaliure => Err(UnresolvableError::InvalidHeader(
                block_hash.clone(),
                out_point.cloned(),
            )),
            HeaderStatus::Live(header) => Ok(header),
        }
    };
    for dep in transaction.deps_iter() {
        match dep {
            CellDep::Cell(out_point) => {
                if let Some(cell_meta) = resolve_cell(&out_point, false)? {
                    resolved_deps.push(ResolvedDep::cell_only(*cell_meta));
                }
            }
            CellDep::CellWithHeader(out_point, block_hash) => {
                let header = resolve_header(&block_hash, Some(&out_point))?;
                if let Some(cell_meta) = resolve_cell(&out_point, false)? {
                    resolved_deps.push(ResolvedDep::cell_and_header(*cell_meta, *header));
                }
            }
            CellDep::DepGroup(out_point) => {
                let sub_resolved_deps = resolve_dep_group(out_point, &mut resolve_cell)?;
                resolved_deps.extend(sub_resolved_deps);
            }
            CellDep::Header(block_hash) => {
                let header = resolve_header(&block_hash, None)?;
                resolved_deps.push(ResolvedDep::header_only(*header));
            }
        }
    }

    if !unknown_out_points.is_empty() {
        Err(UnresolvableError::Unknown(unknown_out_points))
    } else {
        seen_inputs.extend(current_inputs);
        Ok(ResolvedTransaction {
            transaction,
            resolved_inputs,
            resolved_deps,
        })
    }
}

impl<'a> ResolvedTransaction<'a> {
    // cellbase will be resolved with empty input cells, we can use low cost check here:
    pub fn is_cellbase(&self) -> bool {
        self.resolved_inputs.is_empty()
    }

    pub fn inputs_capacity(&self) -> CapacityResult<Capacity> {
        self.resolved_inputs
            .iter()
            .map(|o| o.cell().capacity())
            .try_fold(Capacity::zero(), Capacity::safe_add)
    }

    pub fn outputs_capacity(&self) -> CapacityResult<Capacity> {
        self.transaction.outputs_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::super::block::{Block, BlockBuilder};
    use super::super::script::Script;
    use super::super::transaction::{CellInput, OutPoint, TransactionBuilder};
    use super::*;
    use crate::{capacity_bytes, Bytes, Capacity};
    use numext_fixed_hash::{h256, H256};
    use std::collections::HashMap;

    #[derive(Default)]
    struct CellMemoryDb {
        cells: HashMap<OutPoint, Option<CellMeta>>,
    }
    impl CellProvider for CellMemoryDb {
        fn cell(&self, o: &OutPoint, _with_data: bool) -> CellStatus {
            match self.cells.get(o) {
                Some(&Some(ref cell_meta)) => CellStatus::live_cell(cell_meta.clone()),
                Some(&None) => CellStatus::Dead,
                None => CellStatus::Unknown,
            }
        }
    }

    fn generate_dummy_cell_meta_with_info(out_point: OutPoint, data: Bytes) -> CellMeta {
        let cell_output = CellOutput {
            capacity: capacity_bytes!(2),
            data_hash: CellOutput::calculate_data_hash(&data),
            lock: Script::default(),
            type_: None,
        };
        CellMeta {
            block_info: Some(BlockInfo {
                number: 1,
                epoch: 1,
                hash: H256::zero(),
            }),
            cell_output,
            out_point,
            cellbase: false,
            data_bytes: data.len() as u64,
            mem_cell_data: Some(data),
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

    fn generate_block(txs: Vec<Transaction>) -> Block {
        BlockBuilder::default().transactions(txs).build()
    }

    #[test]
    fn cell_provider_trait_works() {
        let mut db = CellMemoryDb::default();

        let p1 = OutPoint {
            tx_hash: H256::zero(),
            index: 1,
        };
        let p2 = OutPoint {
            tx_hash: H256::zero(),
            index: 2,
        };
        let p3 = OutPoint {
            tx_hash: H256::zero(),
            index: 3,
        };
        let o = generate_dummy_cell_meta();

        db.cells.insert(p1.clone(), Some(o.clone()));
        db.cells.insert(p2.clone(), None);

        assert_eq!(CellStatus::Live(Box::new(o)), db.cell(&p1, false));
        assert_eq!(CellStatus::Dead, db.cell(&p2, false));
        assert_eq!(CellStatus::Unknown, db.cell(&p3, false));
    }

    #[test]
    fn resolve_transaction_should_resolve_dep_group() {
        let mut cell_provider = CellMemoryDb::default();
        let header_provider = BlockHeadersProvider::default();

        let op_dep = OutPoint::new(H256::zero(), 72);
        let op_1 = OutPoint::new(h256!("0x13"), 1);
        let op_2 = OutPoint::new(h256!("0x23"), 2);
        let op_3 = OutPoint::new(h256!("0x33"), 3);

        for op in &[&op_1, &op_2, &op_3] {
            cell_provider.cells.insert(
                (*op).clone(),
                Some(generate_dummy_cell_meta_with_out_point((*op).clone())),
            );
        }
        let cell_data = [
            op_1.to_group_data(),
            op_2.to_group_data(),
            op_3.to_group_data(),
        ]
        .concat();
        let dep_group_cell = generate_dummy_cell_meta_with_data(Bytes::from(cell_data));
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::DepGroup(op_dep);

        let transaction = TransactionBuilder::default().dep(dep).build();
        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        )
        .unwrap();

        assert_eq!(result.resolved_deps.len(), 3);
        assert_eq!(
            result.resolved_deps[0].cell().as_ref().unwrap().out_point,
            op_1
        );
        assert_eq!(
            result.resolved_deps[1].cell().as_ref().unwrap().out_point,
            op_2
        );
        assert_eq!(
            result.resolved_deps[2].cell().as_ref().unwrap().out_point,
            op_3
        );
    }

    #[test]
    fn resolve_transaction_resolve_dep_group_failed_because_invalid_data() {
        let mut cell_provider = CellMemoryDb::default();
        let header_provider = BlockHeadersProvider::default();

        let op_dep = OutPoint::new(H256::zero(), 72);
        let cell_data = Bytes::from("this is invalid data");
        let dep_group_cell = generate_dummy_cell_meta_with_data(cell_data);
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::DepGroup(op_dep.clone());

        let transaction = TransactionBuilder::default().dep(dep).build();
        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        );
        assert_eq!(
            result.unwrap_err(),
            UnresolvableError::InvalidDepGroup(op_dep)
        );
    }

    #[test]
    fn resolve_transaction_resolve_dep_group_failed_because_unknown_sub_cell() {
        let mut cell_provider = CellMemoryDb::default();
        let header_provider = BlockHeadersProvider::default();

        let op_unknown = OutPoint::new(h256!("0x45"), 5);
        let op_dep = OutPoint::new(H256::zero(), 72);
        let cell_data = Bytes::from(op_unknown.to_group_data());
        let dep_group_cell = generate_dummy_cell_meta_with_data(cell_data);
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::DepGroup(op_dep.clone());

        let transaction = TransactionBuilder::default().dep(dep).build();
        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        );
        assert_eq!(
            result.unwrap_err(),
            UnresolvableError::Unknown(vec![op_unknown])
        );
    }

    #[test]
    fn resolve_transaction_should_resolve_header_only_out_point() {
        let cell_provider = CellMemoryDb::default();
        let mut header_provider = BlockHeadersProvider::default();

        let block = generate_block(vec![]);
        let header_hash = block.header().hash();

        header_provider.push_attached(&block);

        let dep = CellDep::Header(header_hash.clone());
        let transaction = TransactionBuilder::default().dep(dep).build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        )
        .unwrap();

        assert!(result.resolved_deps[0].cell().is_none());
        assert_eq!(result.resolved_deps[0].header(), Some(block.header()));
    }

    #[test]
    fn resolve_transaction_should_resolve_both_header_and_cell() {
        let mut cell_provider = CellMemoryDb::default();
        let mut header_provider = BlockHeadersProvider::default();

        let block = generate_block(vec![]);
        let header_hash = block.header().hash();
        let out_point = OutPoint::new(h256!("0x2"), 3);
        let dep = CellDep::CellWithHeader(out_point.clone(), header_hash.clone());

        cell_provider
            .cells
            .insert(out_point.clone(), Some(generate_dummy_cell_meta()));
        header_provider.push_attached(&block);
        header_provider
            .insert_attached_transaction_block(out_point.clone().tx_hash, header_hash.clone());

        let transaction = TransactionBuilder::default().dep(dep).build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        )
        .unwrap();

        assert!(result.resolved_deps[0].cell().is_some());
        assert_eq!(result.resolved_deps[0].header(), Some(block.header()));
    }

    #[test]
    fn resolve_transaction_should_test_header_includes_cell() {
        let mut cell_provider = CellMemoryDb::default();
        let mut header_provider = BlockHeadersProvider::default();

        let block = generate_block(vec![]);
        let header_hash = block.header().hash();
        let out_point = OutPoint::new(h256!("0x2"), 3);
        let dep = CellDep::CellWithHeader(out_point.clone(), header_hash.clone());

        cell_provider
            .cells
            .insert(out_point.clone(), Some(generate_dummy_cell_meta()));
        header_provider.push_attached(&block);

        let transaction = TransactionBuilder::default().dep(dep).build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        );

        assert_eq!(
            result.err(),
            Some(UnresolvableError::InvalidHeader(
                header_hash.clone(),
                Some(out_point)
            ))
        );
    }

    #[test]
    fn resolve_transaction_should_reject_incorrect_order_txs() {
        let out_point = OutPoint::new(h256!("0x2"), 3);

        let tx1 = TransactionBuilder::default()
            .input(CellInput::new(out_point.clone(), 0))
            .output(CellOutput::new(
                capacity_bytes!(2),
                H256::zero(),
                Script::default(),
                None,
            ))
            .output_data(Bytes::new())
            .build();

        let tx2 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx1.hash().to_owned(), 0), 0))
            .build();

        let dep = CellDep::Cell(OutPoint::new(tx1.hash().to_owned(), 0));
        let tx3 = TransactionBuilder::default().dep(dep).build();

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
            let block = generate_block(vec![tx2.clone(), tx1.clone()]);
            let provider = BlockCellProvider::new(&block);

            assert_eq!(
                provider.err(),
                Some(UnresolvableError::OutOfOrder(OutPoint::new(
                    tx1.hash().to_owned(),
                    0
                )))
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
            let block = generate_block(vec![tx3.clone(), tx1.clone()]);
            let provider = BlockCellProvider::new(&block);

            assert_eq!(
                provider.err(),
                Some(UnresolvableError::OutOfOrder(OutPoint::new(
                    tx1.hash().to_owned(),
                    0
                )))
            );
        }
    }

    #[test]
    fn resolve_transaction_should_allow_dep_cell_in_current_tx_input() {
        let mut cell_provider = CellMemoryDb::default();
        let header_provider = BlockHeadersProvider::default();

        let out_point = OutPoint::new(h256!("0x2"), 3);

        let dummy_cell_meta = generate_dummy_cell_meta();
        cell_provider
            .cells
            .insert(out_point.clone(), Some(dummy_cell_meta.clone()));

        let dep = CellDep::Cell(out_point.clone());
        let tx = TransactionBuilder::default()
            .input(CellInput::new(out_point.clone(), 0))
            .dep(dep)
            .build();

        let mut seen_inputs = FnvHashSet::default();
        let rtx =
            resolve_transaction(&tx, &mut seen_inputs, &cell_provider, &header_provider).unwrap();

        assert_eq!(
            rtx.resolved_deps[0],
            ResolvedDep::cell_only(dummy_cell_meta),
        );
    }

    #[test]
    fn resolve_transaction_should_reject_dep_cell_consumed_by_previous_input() {
        let mut cell_provider = CellMemoryDb::default();
        let header_provider = BlockHeadersProvider::default();

        let out_point = OutPoint::new(h256!("0x2"), 3);

        cell_provider
            .cells
            .insert(out_point.clone(), Some(generate_dummy_cell_meta()));

        // tx1 dep
        // tx2 input consumed
        // ok
        {
            let dep = CellDep::Cell(out_point.clone());
            let tx1 = TransactionBuilder::default().dep(dep).build();
            let tx2 = TransactionBuilder::default()
                .input(CellInput::new(out_point.clone(), 0))
                .build();

            let mut seen_inputs = FnvHashSet::default();
            let result1 =
                resolve_transaction(&tx1, &mut seen_inputs, &cell_provider, &header_provider);
            assert!(result1.is_ok());

            let result2 =
                resolve_transaction(&tx2, &mut seen_inputs, &cell_provider, &header_provider);
            assert!(result2.is_ok());
        }

        // tx1 input consumed
        // tx2 dep
        // tx2 resolve err
        {
            let tx1 = TransactionBuilder::default()
                .input(CellInput::new(out_point.clone(), 0))
                .build();

            let dep = CellDep::Cell(out_point.clone());
            let tx2 = TransactionBuilder::default().dep(dep).build();

            let mut seen_inputs = FnvHashSet::default();
            let result1 =
                resolve_transaction(&tx1, &mut seen_inputs, &cell_provider, &header_provider);

            assert!(result1.is_ok());

            let result2 =
                resolve_transaction(&tx2, &mut seen_inputs, &cell_provider, &header_provider);

            assert_eq!(
                result2.err(),
                Some(UnresolvableError::Dead(out_point.clone()))
            );
        }
    }
}
