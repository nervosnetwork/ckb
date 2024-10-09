use ckb_error::assert_error_eq;
use std::collections::{HashMap, HashSet};

use crate::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{
            resolve_transaction, BlockCellProvider, CellMeta, CellProvider, CellStatus,
            HeaderChecker,
        },
        error::OutPointError,
        BlockBuilder, BlockView, Capacity, DepType, EpochNumberWithFraction, TransactionBuilder,
        TransactionInfo, TransactionView,
    },
    h256,
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
};

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
        if !self.detached_indices.contains(block_hash) && self.attached_indices.contains(block_hash)
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
            Some(Some(cell_meta)) => CellStatus::live_cell(cell_meta.clone()),
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
    let rtx = resolve_transaction(tx, &mut seen_inputs, &cell_provider, &header_checker).unwrap();

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
        let result1 = resolve_transaction(tx1, &mut seen_inputs, &cell_provider, &header_checker);
        assert!(result1.is_ok());

        let result2 = resolve_transaction(tx2, &mut seen_inputs, &cell_provider, &header_checker);
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
        let result1 = resolve_transaction(tx1, &mut seen_inputs, &cell_provider, &header_checker);

        assert!(result1.is_ok());

        let result2 = resolve_transaction(tx2, &mut seen_inputs, &cell_provider, &header_checker);

        assert_error_eq!(result2.unwrap_err(), OutPointError::Dead(out_point));
    }
}
