use crate::block::Block;
use crate::header::Header;
use crate::transaction::{CellOutPoint, CellOutput, OutPoint, Transaction};
use crate::Capacity;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Eq, PartialEq, Debug, Default, Deserialize, Serialize)]
pub struct CellMeta {
    #[serde(skip)]
    pub cell_output: Option<CellOutput>,
    pub out_point: CellOutPoint,
    pub block_number: Option<u64>,
    pub cellbase: bool,
    pub capacity: Capacity,
    pub data_hash: Option<H256>,
}

impl From<&CellOutput> for CellMeta {
    fn from(output: &CellOutput) -> Self {
        CellMeta {
            cell_output: Some(output.clone()),
            capacity: output.capacity,
            ..Default::default()
        }
    }
}

impl CellMeta {
    pub fn is_cellbase(&self) -> bool {
        self.cellbase
    }

    pub fn capacity(&self) -> Capacity {
        self.capacity
    }

    pub fn data_hash(&self) -> Option<&H256> {
        self.data_hash.as_ref()
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
    /// OutPoint doesn't contain reference to a cell.
    Unspecified,
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

    pub fn is_unspecified(&self) -> bool {
        self == &CellStatus::Unspecified
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
    /// OutPoint doesn't contain reference to a header.
    Unspecified,
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

    pub fn is_unspecified(&self) -> bool {
        self == &HeaderStatus::Unspecified
    }
}

#[derive(Debug)]
pub struct ResolvedOutPoint {
    pub cell: Option<CellMeta>,
    pub header: Option<Box<Header>>,
}

impl ResolvedOutPoint {
    pub fn cell_only(cell: CellMeta) -> ResolvedOutPoint {
        ResolvedOutPoint {
            cell: Some(cell),
            header: None,
        }
    }

    pub fn header_only(header: Header) -> ResolvedOutPoint {
        ResolvedOutPoint {
            cell: None,
            header: Some(Box::new(header)),
        }
    }

    pub fn cell_and_header(cell: CellMeta, header: Header) -> ResolvedOutPoint {
        ResolvedOutPoint {
            cell: Some(cell),
            header: Some(Box::new(header)),
        }
    }

    pub fn cell(&self) -> Option<&CellMeta> {
        self.cell.as_ref()
    }

    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref().map(|h| &**h)
    }
}

/// Transaction with resolved input cells.
#[derive(Debug)]
pub struct ResolvedTransaction<'a> {
    pub transaction: &'a Transaction,
    pub dep_cells: Vec<ResolvedOutPoint>,
    pub input_cells: Vec<ResolvedOutPoint>,
}

pub trait CellProvider {
    fn cell(&self, out_point: &OutPoint) -> CellStatus;
}

pub struct OverlayCellProvider<'a> {
    overlay: &'a CellProvider,
    cell_provider: &'a CellProvider,
}

impl<'a> OverlayCellProvider<'a> {
    pub fn new(overlay: &'a CellProvider, cell_provider: &'a CellProvider) -> Self {
        Self {
            overlay,
            cell_provider,
        }
    }
}

impl<'a> CellProvider for OverlayCellProvider<'a> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        match self.overlay.cell(out_point) {
            CellStatus::Live(cell_meta) => CellStatus::Live(cell_meta),
            CellStatus::Dead => CellStatus::Dead,
            CellStatus::Unknown => self.cell_provider.cell(out_point),
            CellStatus::Unspecified => CellStatus::Unspecified,
        }
    }
}

pub struct BlockCellProvider<'a> {
    output_indices: FnvHashMap<H256, usize>,
    block: &'a Block,
}

impl<'a> BlockCellProvider<'a> {
    pub fn new(block: &'a Block) -> Self {
        let output_indices = block
            .transactions()
            .iter()
            .enumerate()
            .map(|(idx, tx)| (tx.hash().to_owned(), idx))
            .collect();
        Self {
            output_indices,
            block,
        }
    }
}

impl<'a> CellProvider for BlockCellProvider<'a> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if out_point.cell.is_none() {
            return CellStatus::Unspecified;
        }
        let out_point = out_point.cell.as_ref().unwrap();

        self.output_indices
            .get(&out_point.tx_hash)
            .and_then(|i| {
                self.block.transactions()[*i]
                    .outputs()
                    .get(out_point.index as usize)
                    .map(|output| {
                        CellStatus::live_cell(CellMeta {
                            cell_output: Some(output.clone()),
                            out_point: out_point.to_owned(),
                            data_hash: None,
                            capacity: output.capacity,
                            block_number: Some(self.block.header().number()),
                            cellbase: *i == 0,
                        })
                    })
            })
            .unwrap_or_else(|| CellStatus::Unknown)
    }
}

pub trait HeaderProvider {
    fn header(&self, out_point: &OutPoint) -> HeaderStatus;
}

pub struct OverlayHeaderProvider<'a, O, HP> {
    overlay: &'a O,
    header_provider: &'a HP,
}

impl<'a, O, HP> OverlayHeaderProvider<'a, O, HP> {
    pub fn new(overlay: &'a O, header_provider: &'a HP) -> Self {
        OverlayHeaderProvider {
            overlay,
            header_provider,
        }
    }
}

impl<'a, O, HP> HeaderProvider for OverlayHeaderProvider<'a, O, HP>
where
    O: HeaderProvider,
    HP: HeaderProvider,
{
    fn header(&self, out_point: &OutPoint) -> HeaderStatus {
        match self.overlay.header(out_point) {
            HeaderStatus::Live(h) => HeaderStatus::Live(h),
            HeaderStatus::InclusionFaliure => HeaderStatus::InclusionFaliure,
            HeaderStatus::Unknown => self.header_provider.header(out_point),
            HeaderStatus::Unspecified => HeaderStatus::Unspecified,
        }
    }
}

#[derive(Default)]
pub struct BlockHeadersProvider {
    attached_indices: FnvHashMap<H256, Header>,
    attached_transaction_blocks: FnvHashMap<H256, H256>,
    detached_indices: FnvHashMap<H256, Header>,
}

impl<'a> BlockHeadersProvider {
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
}

impl HeaderProvider for BlockHeadersProvider {
    fn header(&self, out_point: &OutPoint) -> HeaderStatus {
        if let Some(block_hash) = &out_point.block_hash {
            if self.detached_indices.contains_key(&block_hash) {
                return HeaderStatus::Unknown;
            }
            match self.attached_indices.get(&block_hash) {
                Some(header) => {
                    if let Some(cell_out_point) = &out_point.cell {
                        self.attached_transaction_blocks
                            .get(&cell_out_point.tx_hash)
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
        } else {
            HeaderStatus::Unspecified
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UnresolvableError {
    // OutPoint is empty
    Empty,
    // OutPoint is used as input, but a cell is not specified
    UnspecifiedInputCell(OutPoint),
    // OutPoint specifies an invalid header, this could be due to either
    // of the following 2 reasons:
    // 1. Specified header doesn't exist on chain.
    // 2. OutPoint specifies both header and cell, but the specified cell
    // is not included in the specified block header.
    InvalidHeader(OutPoint),
    Dead(OutPoint),
    Unknown(Vec<OutPoint>),
}

pub fn resolve_transaction<'a, CP: CellProvider, HP: HeaderProvider>(
    transaction: &'a Transaction,
    seen_inputs: &mut FnvHashSet<OutPoint>,
    cell_provider: &CP,
    header_provider: &HP,
) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
    let (mut unknown_out_points, mut input_cells, mut dep_cells) = (
        Vec::new(),
        Vec::with_capacity(transaction.inputs().len()),
        Vec::with_capacity(transaction.deps().len()),
    );

    // skip resolve input of cellbase
    if !transaction.is_cellbase() {
        for out_point in transaction.input_pts() {
            let (cell_status, header_status) = if seen_inputs.insert(out_point.clone()) {
                (
                    cell_provider.cell(&out_point),
                    header_provider.header(&out_point),
                )
            } else {
                (CellStatus::Dead, HeaderStatus::Unknown)
            };

            match (cell_status, header_status) {
                (CellStatus::Dead, _) => {
                    return Err(UnresolvableError::Dead(out_point.clone()));
                }
                (CellStatus::Unknown, _) => {
                    unknown_out_points.push(out_point.clone());
                }
                // Input cell must exist
                (CellStatus::Unspecified, _) => {
                    return Err(UnresolvableError::UnspecifiedInputCell(out_point.clone()));
                }
                (_, HeaderStatus::Unknown) => {
                    // TODO: should we change transaction pool so transactions
                    // with unknown header can be included as orphans, waiting
                    // for the correct block header to enable it?
                    return Err(UnresolvableError::InvalidHeader(out_point.clone()));
                }
                (_, HeaderStatus::InclusionFaliure) => {
                    return Err(UnresolvableError::InvalidHeader(out_point.clone()));
                }
                (CellStatus::Live(cell_meta), HeaderStatus::Live(header)) => {
                    input_cells.push(ResolvedOutPoint::cell_and_header(*cell_meta, *header));
                }
                (CellStatus::Live(cell_meta), HeaderStatus::Unspecified) => {
                    input_cells.push(ResolvedOutPoint::cell_only(*cell_meta));
                }
            }
        }
    }

    for out_point in transaction.dep_pts() {
        let cell_status = cell_provider.cell(&out_point);
        let header_status = header_provider.header(&out_point);

        match (cell_status, header_status) {
            (CellStatus::Dead, _) => {
                return Err(UnresolvableError::Dead(out_point.clone()));
            }
            (CellStatus::Unknown, _) => {
                unknown_out_points.push(out_point.clone());
            }
            (_, HeaderStatus::Unknown) => {
                // TODO: should we change transaction pool so transactions
                // with unknown header can be included as orphans, waiting
                // for the correct block header to enable it?
                return Err(UnresolvableError::InvalidHeader(out_point.clone()));
            }
            (_, HeaderStatus::InclusionFaliure) => {
                return Err(UnresolvableError::InvalidHeader(out_point.clone()));
            }
            (CellStatus::Live(cell_meta), HeaderStatus::Live(header)) => {
                dep_cells.push(ResolvedOutPoint::cell_and_header(*cell_meta, *header));
            }
            (CellStatus::Live(cell_meta), HeaderStatus::Unspecified) => {
                dep_cells.push(ResolvedOutPoint::cell_only(*cell_meta));
            }
            (CellStatus::Unspecified, HeaderStatus::Live(header)) => {
                dep_cells.push(ResolvedOutPoint::header_only(*header));
            }
            (CellStatus::Unspecified, HeaderStatus::Unspecified) => {
                return Err(UnresolvableError::Empty);
            }
        }
    }

    if !unknown_out_points.is_empty() {
        Err(UnresolvableError::Unknown(unknown_out_points))
    } else {
        Ok(ResolvedTransaction {
            transaction,
            input_cells,
            dep_cells,
        })
    }
}

impl<'a> ResolvedTransaction<'a> {
    // cellbase will be resolved with empty input cells, we can use low cost check here:
    pub fn is_cellbase(&self) -> bool {
        self.input_cells.is_empty()
    }

    pub fn fee(&self) -> ::occupied_capacity::Result<Capacity> {
        self.inputs_capacity().and_then(|x| {
            self.transaction.outputs_capacity().and_then(|y| {
                if x > y {
                    x.safe_sub(y)
                } else {
                    Ok(Capacity::zero())
                }
            })
        })
    }

    pub fn inputs_capacity(&self) -> ::occupied_capacity::Result<Capacity> {
        self.input_cells
            .iter()
            .map(|o| {
                o.cell
                    .as_ref()
                    .map_or_else(Capacity::zero, CellMeta::capacity)
            })
            .try_fold(Capacity::zero(), Capacity::safe_add)
    }
}

#[cfg(test)]
mod tests {
    use super::super::header::{Header, HeaderBuilder};
    use super::super::script::Script;
    use super::super::transaction::{CellInput, CellOutPoint, OutPoint, TransactionBuilder};
    use super::*;
    use crate::{capacity_bytes, Bytes, Capacity};
    use numext_fixed_hash::{h256, H256};
    use std::collections::HashMap;

    #[derive(Default)]
    struct CellMemoryDb {
        cells: HashMap<CellOutPoint, Option<CellMeta>>,
    }
    impl CellProvider for CellMemoryDb {
        fn cell(&self, o: &OutPoint) -> CellStatus {
            if o.cell.is_none() {
                return CellStatus::Unspecified;
            }

            match self.cells.get(o.cell.as_ref().unwrap()) {
                Some(&Some(ref cell_meta)) => CellStatus::live_cell(cell_meta.clone()),
                Some(&None) => CellStatus::Dead,
                None => CellStatus::Unknown,
            }
        }
    }

    #[derive(Default)]
    struct HeaderMemoryDb {
        headers: HashMap<H256, Header>,
        tx_headers: HashMap<CellOutPoint, H256>,
    }
    impl HeaderMemoryDb {
        fn insert_header(&mut self, header: Header) {
            self.headers.insert(header.hash().clone(), header);
        }
    }
    impl HeaderProvider for HeaderMemoryDb {
        fn header(&self, o: &OutPoint) -> HeaderStatus {
            if o.block_hash.is_none() {
                return HeaderStatus::Unspecified;
            }

            match self.headers.get(o.block_hash.as_ref().unwrap()) {
                Some(header) => {
                    if o.cell.is_some() {
                        self.tx_headers.get(o.cell.as_ref().unwrap()).map_or(
                            HeaderStatus::InclusionFaliure,
                            |h| {
                                if h == header.hash() {
                                    HeaderStatus::live_header(header.clone())
                                } else {
                                    HeaderStatus::InclusionFaliure
                                }
                            },
                        )
                    } else {
                        HeaderStatus::live_header(header.clone())
                    }
                }
                None => HeaderStatus::Unknown,
            }
        }
    }

    fn generate_dummy_cell_meta() -> CellMeta {
        let cell_output = CellOutput {
            capacity: capacity_bytes!(2),
            data: Bytes::default(),
            lock: Script::default(),
            type_: None,
        };
        CellMeta {
            block_number: Some(1),
            capacity: cell_output.capacity,
            data_hash: Some(cell_output.data_hash()),
            cell_output: Some(cell_output),
            out_point: CellOutPoint {
                tx_hash: Default::default(),
                index: 0,
            },
            cellbase: false,
        }
    }

    #[test]
    fn cell_provider_trait_works() {
        let mut db = CellMemoryDb::default();

        let p1 = OutPoint {
            block_hash: None,
            cell: Some(CellOutPoint {
                tx_hash: H256::zero(),
                index: 1,
            }),
        };
        let p2 = OutPoint {
            block_hash: None,
            cell: Some(CellOutPoint {
                tx_hash: H256::zero(),
                index: 2,
            }),
        };
        let p3 = OutPoint {
            block_hash: None,
            cell: Some(CellOutPoint {
                tx_hash: H256::zero(),
                index: 3,
            }),
        };
        let o = generate_dummy_cell_meta();

        db.cells.insert(p1.cell.clone().unwrap(), Some(o.clone()));
        db.cells.insert(p2.cell.clone().unwrap(), None);

        assert_eq!(CellStatus::Live(Box::new(o)), db.cell(&p1));
        assert_eq!(CellStatus::Dead, db.cell(&p2));
        assert_eq!(CellStatus::Unknown, db.cell(&p3));
    }

    fn generate_header(parent_hash: H256) -> Header {
        HeaderBuilder::default().parent_hash(parent_hash).build()
    }

    #[test]
    fn resolve_transaction_should_resolve_header_only_out_point() {
        let cell_provider = CellMemoryDb::default();
        let mut header_provider = HeaderMemoryDb::default();

        let header = generate_header(h256!("0x1"));
        let header_hash = header.hash();

        header_provider.insert_header(header.clone());

        let out_point = OutPoint::new_block_hash(header_hash.clone());
        let transaction = TransactionBuilder::default().dep(out_point).build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        )
        .unwrap();

        assert!(result.dep_cells[0].cell.is_none());
        assert_eq!(result.dep_cells[0].header, Some(Box::new(header)));
    }

    #[test]
    fn resolve_transaction_should_reject_input_without_cells() {
        let cell_provider = CellMemoryDb::default();
        let mut header_provider = HeaderMemoryDb::default();

        let header = generate_header(h256!("0x1"));
        let header_hash = header.hash();

        header_provider.insert_header(header.clone());

        let out_point = OutPoint::new_block_hash(header_hash.clone());
        let transaction = TransactionBuilder::default()
            .input(CellInput::new(out_point, 0, vec![]))
            .build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        );

        assert!(result.is_err());
    }

    #[test]
    fn resolve_transaction_should_resolve_both_header_and_cell() {
        let mut cell_provider = CellMemoryDb::default();
        let mut header_provider = HeaderMemoryDb::default();

        let header = generate_header(h256!("0x1"));
        let header_hash = header.hash();
        let out_point = OutPoint::new(header_hash.clone(), h256!("0x2"), 3);

        cell_provider.cells.insert(
            out_point.cell.clone().unwrap(),
            Some(generate_dummy_cell_meta()),
        );
        header_provider.insert_header(header.clone());
        header_provider
            .tx_headers
            .insert(out_point.cell.clone().unwrap(), header_hash.clone());

        let transaction = TransactionBuilder::default().dep(out_point).build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        )
        .unwrap();

        assert!(result.dep_cells[0].cell.is_some());
        assert_eq!(result.dep_cells[0].header, Some(Box::new(header)));
    }

    #[test]
    fn resolve_transaction_should_test_header_includes_cell() {
        let mut cell_provider = CellMemoryDb::default();
        let mut header_provider = HeaderMemoryDb::default();

        let header = generate_header(h256!("0x1"));
        let header_hash = header.hash();
        let out_point = OutPoint::new(header_hash.clone(), h256!("0x2"), 3);

        cell_provider.cells.insert(
            out_point.cell.clone().unwrap(),
            Some(generate_dummy_cell_meta()),
        );
        header_provider.insert_header(header.clone());

        let transaction = TransactionBuilder::default().dep(out_point).build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        );

        assert!(result.is_err());
    }

    #[test]
    fn resolve_transaction_should_reject_empty_out_point() {
        let mut cell_provider = CellMemoryDb::default();
        let mut header_provider = HeaderMemoryDb::default();

        let header = generate_header(h256!("0x1"));
        let header_hash = header.hash();
        let out_point = OutPoint::new(header_hash.clone(), h256!("0x2"), 3);

        cell_provider.cells.insert(
            out_point.cell.clone().unwrap(),
            Some(generate_dummy_cell_meta()),
        );
        header_provider.insert_header(header.clone());
        header_provider
            .tx_headers
            .insert(out_point.cell.clone().unwrap(), header_hash.clone());

        let transaction = TransactionBuilder::default()
            .dep(OutPoint::default())
            .build();

        let mut seen_inputs = FnvHashSet::default();
        let result = resolve_transaction(
            &transaction,
            &mut seen_inputs,
            &cell_provider,
            &header_provider,
        );

        assert!(result.is_err());
    }
}
