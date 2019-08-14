use crate::block::Block;
use crate::extras::TransactionInfo;
use crate::transaction::{CellOutput, OutPoint, Transaction, OUT_POINT_LEN};
use crate::{Bytes, Capacity};
use ckb_occupied_capacity::Result as CapacityResult;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashSet;
use std::convert::TryInto;
use std::fmt;

#[derive(Clone, Eq, PartialEq, Default, Deserialize, Serialize)]
pub struct CellMeta {
    #[serde(skip)]
    pub cell_output: CellOutput,
    pub out_point: OutPoint,
    pub transaction_info: Option<TransactionInfo>,
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
    transaction_info: Option<TransactionInfo>,
    data_bytes: u64,
    mem_cell_data: Option<Bytes>,
}

impl CellMetaBuilder {
    pub fn from_cell_meta(cell_meta: CellMeta) -> Self {
        let CellMeta {
            cell_output,
            out_point,
            transaction_info,
            data_bytes,
            mem_cell_data,
        } = cell_meta;
        Self {
            cell_output,
            out_point,
            transaction_info,
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

    pub fn transaction_info(mut self, transaction_info: TransactionInfo) -> Self {
        self.transaction_info = Some(transaction_info);
        self
    }

    pub fn build(self) -> CellMeta {
        let Self {
            cell_output,
            out_point,
            transaction_info,
            data_bytes,
            mem_cell_data,
        } = self;
        CellMeta {
            cell_output,
            out_point,
            transaction_info,
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
            .field("transaction_info", &self.transaction_info)
            .field("data_bytes", &self.data_bytes)
            .finish()
    }
}

impl CellMeta {
    pub fn is_cellbase(&self) -> bool {
        self.transaction_info
            .as_ref()
            .map(|info| info.index == 0)
            .unwrap_or(false)
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

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction<'a> {
    pub transaction: &'a Transaction,
    pub resolved_cell_deps: Vec<CellMeta>,
    pub resolved_inputs: Vec<CellMeta>,
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
            for dep in tx.cell_deps_iter() {
                if let Some(output_idx) = output_indices.get(&dep.out_point().tx_hash) {
                    if *output_idx >= idx {
                        return Err(UnresolvableError::OutOfOrder(dep.out_point().clone()));
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
                            transaction_info: Some(TransactionInfo {
                                block_number: self.block.header().number(),
                                block_epoch: self.block.header().epoch(),
                                block_hash: self.block.header().hash().to_owned(),
                                index: *i,
                            }),
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

pub trait HeaderChecker {
    /// Check if header in main chain
    fn is_valid(&self, block_hash: &H256) -> bool;
}

#[derive(Default)]
pub struct BlockHeadersChecker {
    attached_indices: HashSet<H256>,
    detached_indices: HashSet<H256>,
}

impl BlockHeadersChecker {
    pub fn push_attached(&mut self, block_hash: H256) {
        self.attached_indices.insert(block_hash);
    }

    pub fn push_detached(&mut self, block_hash: H256) {
        self.detached_indices.insert(block_hash);
    }
}

impl HeaderChecker for BlockHeadersChecker {
    fn is_valid(&self, block_hash: &H256) -> bool {
        !self.detached_indices.contains(block_hash) && self.attached_indices.contains(block_hash)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UnresolvableError {
    /// The header is not in main chain
    InvalidHeader(H256),
    /// Invalid dep group cell data length
    InvalidDepGroup(OutPoint),
    Dead(OutPoint),
    Unknown(Vec<OutPoint>),
    OutOfOrder(OutPoint),
}

/// Gather all cell dep out points and resolved dep group out points
pub fn get_related_dep_out_points<F: Fn(&OutPoint) -> Option<Bytes>>(
    tx: &Transaction,
    get_cell_data: F,
) -> Result<Vec<OutPoint>, String> {
    tx.cell_deps_iter().try_fold(
        Vec::with_capacity(tx.cell_deps().len()),
        |mut out_points, dep| {
            let out_point = dep.out_point();
            if dep.is_dep_group() {
                let data = get_cell_data(out_point)
                    .ok_or_else(|| String::from("Can not get cell data"))?;
                let sub_out_points = parse_dep_group_data(&data)
                    .map_err(|len| format!("Invalid data length {}", len))?;
                out_points.extend(sub_out_points);
            }
            out_points.push(out_point.clone());
            Ok(out_points)
        },
    )
}

pub fn parse_dep_group_data(data: &[u8]) -> Result<Vec<OutPoint>, usize> {
    if data.is_empty() || data.len() % OUT_POINT_LEN != 0 {
        return Err(data.len());
    }

    Ok(data
        .chunks_exact(OUT_POINT_LEN)
        .map(|item_data| {
            OutPoint::from_group_data(item_data).expect("parse group item data failed")
        })
        .collect::<Vec<_>>())
}

fn resolve_dep_group<
    F: FnMut(&OutPoint, bool) -> Result<Option<Box<CellMeta>>, UnresolvableError>,
>(
    out_point: &OutPoint,
    mut cell_resolver: F,
) -> Result<Vec<CellMeta>, UnresolvableError> {
    let data = match cell_resolver(out_point, true)? {
        Some(cell_meta) => cell_meta
            .mem_cell_data
            .expect("Load cell meta must with data"),
        None => return Ok(Vec::new()),
    };

    let sub_out_points = parse_dep_group_data(&data)
        .map_err(|_| UnresolvableError::InvalidDepGroup(out_point.clone()))?;
    let mut resolved_deps = Vec::with_capacity(sub_out_points.len());
    for sub_out_point in sub_out_points {
        if let Some(sub_cell_meta) = cell_resolver(&sub_out_point, false)? {
            resolved_deps.push(*sub_cell_meta);
        }
    }
    Ok(resolved_deps)
}

pub fn resolve_transaction<'a, CP: CellProvider, HC: HeaderChecker>(
    transaction: &'a Transaction,
    seen_inputs: &mut FnvHashSet<OutPoint>,
    cell_provider: &CP,
    header_checker: &HC,
) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
    let (mut unknown_out_points, mut resolved_inputs, mut resolved_cell_deps) = (
        Vec::new(),
        Vec::with_capacity(transaction.inputs().len()),
        Vec::with_capacity(transaction.cell_deps().len()),
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
                resolved_inputs.push(*cell_meta);
            }
        }
    }

    for cell_dep in transaction.cell_deps_iter() {
        if cell_dep.is_dep_group() {
            resolved_cell_deps.extend(resolve_dep_group(cell_dep.out_point(), &mut resolve_cell)?);
        } else if let Some(cell_meta) = resolve_cell(cell_dep.out_point(), false)? {
            resolved_cell_deps.push(*cell_meta);
        }
    }

    for block_hash in transaction.header_deps_iter() {
        if !header_checker.is_valid(block_hash) {
            return Err(UnresolvableError::InvalidHeader(block_hash.clone()));
        }
    }

    if !unknown_out_points.is_empty() {
        Err(UnresolvableError::Unknown(unknown_out_points))
    } else {
        seen_inputs.extend(current_inputs);
        Ok(ResolvedTransaction {
            transaction,
            resolved_inputs,
            resolved_cell_deps,
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
            .map(CellMeta::capacity)
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
    use super::super::transaction::{CellDep, CellInput, OutPoint, TransactionBuilder};
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
            transaction_info: Some(TransactionInfo {
                block_number: 1,
                block_epoch: 1,
                block_hash: H256::zero(),
                index: 1,
            }),
            cell_output,
            out_point,
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
        let header_checker = BlockHeadersChecker::default();

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

        let dep = CellDep::new_group(op_dep);

        let transaction = TransactionBuilder::default().cell_dep(dep).build();
        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
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

        let op_dep = OutPoint::new(H256::zero(), 72);
        let cell_data = Bytes::from("this is invalid data");
        let dep_group_cell = generate_dummy_cell_meta_with_data(cell_data);
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::new_group(op_dep.clone());

        let transaction = TransactionBuilder::default().cell_dep(dep).build();
        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        );
        assert_eq!(
            result.unwrap_err(),
            UnresolvableError::InvalidDepGroup(op_dep)
        );
    }

    #[test]
    fn resolve_transaction_resolve_dep_group_failed_because_unknown_sub_cell() {
        let mut cell_provider = CellMemoryDb::default();
        let header_checker = BlockHeadersChecker::default();

        let op_unknown = OutPoint::new(h256!("0x45"), 5);
        let op_dep = OutPoint::new(H256::zero(), 72);
        let cell_data = Bytes::from(op_unknown.to_group_data());
        let dep_group_cell = generate_dummy_cell_meta_with_data(cell_data);
        cell_provider
            .cells
            .insert(op_dep.clone(), Some(dep_group_cell));

        let dep = CellDep::new_group(op_dep.clone());

        let transaction = TransactionBuilder::default().cell_dep(dep).build();
        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        );
        assert_eq!(
            result.unwrap_err(),
            UnresolvableError::Unknown(vec![op_unknown])
        );
    }

    #[test]
    fn resolve_transaction_test_header_deps_all_ok() {
        let cell_provider = CellMemoryDb::default();
        let mut header_checker = BlockHeadersChecker::default();

        let block_hash1 = h256!("0x1111");
        let block_hash2 = h256!("0x2222");

        header_checker.push_attached(block_hash1.clone());
        header_checker.push_attached(block_hash2.clone());

        let transaction = TransactionBuilder::default()
            .header_dep(block_hash1)
            .header_dep(block_hash2)
            .build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
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

        let main_chain_block_hash = h256!("0xaabbcc");
        let invalid_block_hash = h256!("0x3344");

        header_checker.push_attached(main_chain_block_hash.clone());

        let transaction = TransactionBuilder::default()
            .header_dep(main_chain_block_hash.clone())
            .header_dep(invalid_block_hash.clone())
            .build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_checker,
        );

        assert_eq!(
            result.err(),
            Some(UnresolvableError::InvalidHeader(invalid_block_hash))
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

        let tx3 = TransactionBuilder::default()
            .cell_dep(CellDep::new_cell(OutPoint::new(tx1.hash().to_owned(), 0)))
            .build();

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
        let header_checker = BlockHeadersChecker::default();

        let out_point = OutPoint::new(h256!("0x2"), 3);

        let dummy_cell_meta = generate_dummy_cell_meta();
        cell_provider
            .cells
            .insert(out_point.clone(), Some(dummy_cell_meta.clone()));

        let tx = TransactionBuilder::default()
            .input(CellInput::new(out_point.clone(), 0))
            .cell_dep(CellDep::new_cell(out_point.clone()))
            .build();

        let mut seen_inputs = FnvHashSet::default();
        let rtx =
            resolve_transaction(&tx, &mut seen_inputs, &cell_provider, &header_checker).unwrap();

        assert_eq!(rtx.resolved_cell_deps[0], dummy_cell_meta,);
    }

    #[test]
    fn resolve_transaction_should_reject_dep_cell_consumed_by_previous_input() {
        let mut cell_provider = CellMemoryDb::default();
        let header_checker = BlockHeadersChecker::default();

        let out_point = OutPoint::new(h256!("0x2"), 3);

        cell_provider
            .cells
            .insert(out_point.clone(), Some(generate_dummy_cell_meta()));

        // tx1 dep
        // tx2 input consumed
        // ok
        {
            let tx1 = TransactionBuilder::default()
                .cell_dep(CellDep::new_cell(out_point.clone()))
                .build();
            let tx2 = TransactionBuilder::default()
                .input(CellInput::new(out_point.clone(), 0))
                .build();

            let mut seen_inputs = FnvHashSet::default();
            let result1 =
                resolve_transaction(&tx1, &mut seen_inputs, &cell_provider, &header_checker);
            assert!(result1.is_ok());

            let result2 =
                resolve_transaction(&tx2, &mut seen_inputs, &cell_provider, &header_checker);
            assert!(result2.is_ok());
        }

        // tx1 input consumed
        // tx2 dep
        // tx2 resolve err
        {
            let tx1 = TransactionBuilder::default()
                .input(CellInput::new(out_point.clone(), 0))
                .build();

            let tx2 = TransactionBuilder::default()
                .cell_dep(CellDep::new_cell(out_point.clone()))
                .build();

            let mut seen_inputs = FnvHashSet::default();
            let result1 =
                resolve_transaction(&tx1, &mut seen_inputs, &cell_provider, &header_checker);

            assert!(result1.is_ok());

            let result2 =
                resolve_transaction(&tx2, &mut seen_inputs, &cell_provider, &header_checker);

            assert_eq!(
                result2.err(),
                Some(UnresolvableError::Dead(out_point.clone()))
            );
        }
    }
}
