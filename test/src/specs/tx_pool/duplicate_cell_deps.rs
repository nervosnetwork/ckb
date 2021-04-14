use crate::{
    util::{cell::gen_spendable, check::is_transaction_committed, mining::mine_until_bool},
    utils::assert_send_transaction_fail,
    Node, Spec,
};
use ckb_logger::info;
use ckb_types::{
    core::{Capacity, DepType, TransactionBuilder},
    packed,
    prelude::*,
};

pub struct DuplicateCellDeps;

impl Spec for DuplicateCellDeps {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let always_success_bytes: packed::Bytes = node0.always_success_raw_data().pack();
        let always_success_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::bytes(always_success_bytes.len()).unwrap())
            .unwrap();
        let empty_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::shannons(0))
            .unwrap();

        let mut initial_inputs = gen_spendable(node0, 2 + 3 + 6)
            .into_iter()
            .map(|input| packed::CellInput::new(input.out_point, 0));

        info!("warm up: create 2 transactions as code-type cell deps.");
        let dep1_tx = TransactionBuilder::default()
            .cell_dep(node0.always_success_cell_dep())
            .input(initial_inputs.next().unwrap())
            .output(always_success_output.clone())
            .output_data(always_success_bytes.clone())
            .build();
        let dep2_tx = TransactionBuilder::default()
            .cell_dep(node0.always_success_cell_dep())
            .input(initial_inputs.next().unwrap())
            .output(always_success_output)
            .output_data(always_success_bytes)
            .build();
        node0.submit_transaction(&dep1_tx);
        node0.submit_transaction(&dep2_tx);
        mine_until_bool(node0, || is_transaction_committed(node0, &dep1_tx));
        mine_until_bool(node0, || is_transaction_committed(node0, &dep2_tx));

        info!("warm up: create 3 transactions as depgroup-type cell deps.");
        let dep1_op = packed::OutPoint::new(dep1_tx.hash(), 0);
        let dep2_op = packed::OutPoint::new(dep2_tx.hash(), 0);
        let dep3_data = vec![dep1_op.clone()].pack().as_bytes().pack();
        let dep4_data = vec![dep2_op.clone()].pack().as_bytes().pack();
        let dep3_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::bytes(dep3_data.len()).unwrap())
            .unwrap();
        let dep4_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::bytes(dep4_data.len()).unwrap())
            .unwrap();
        let dep3_tx = TransactionBuilder::default()
            .cell_dep(node0.always_success_cell_dep())
            .input(initial_inputs.next().unwrap())
            .output(dep3_output)
            .output_data(dep3_data)
            .build();
        let dep4_tx = TransactionBuilder::default()
            .cell_dep(node0.always_success_cell_dep())
            .input(initial_inputs.next().unwrap())
            .output(dep4_output.clone())
            .output_data(dep4_data.clone())
            .build();
        let dep4b_tx = TransactionBuilder::default()
            .cell_dep(node0.always_success_cell_dep())
            .input(initial_inputs.next().unwrap())
            .output(dep4_output)
            .output_data(dep4_data)
            .build();
        node0.submit_transaction(&dep3_tx);
        node0.submit_transaction(&dep4_tx);
        node0.submit_transaction(&dep4b_tx);
        mine_until_bool(node0, || is_transaction_committed(node0, &dep3_tx));
        mine_until_bool(node0, || is_transaction_committed(node0, &dep4_tx));
        mine_until_bool(node0, || is_transaction_committed(node0, &dep4b_tx));

        info!("warm up: create all cell deps for test.");
        let dep1 = packed::CellDep::new_builder()
            .out_point(dep1_op)
            .dep_type(DepType::Code.into())
            .build();
        let dep2 = packed::CellDep::new_builder()
            .out_point(dep2_op)
            .dep_type(DepType::Code.into())
            .build();
        let dep3 = packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(dep3_tx.hash(), 0))
            .dep_type(DepType::DepGroup.into())
            .build();
        let dep4 = packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(dep4_tx.hash(), 0))
            .dep_type(DepType::DepGroup.into())
            .build();
        let dep4b = packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(dep4b_tx.hash(), 0))
            .dep_type(DepType::DepGroup.into())
            .build();

        {
            info!("test: duplicate code-type cell deps is not allowed.");
            let tx = TransactionBuilder::default()
                .cell_dep(dep1.clone())
                .cell_dep(dep1.clone())
                .input(initial_inputs.next().unwrap())
                .output(empty_output.clone())
                .output_data(Default::default())
                .build();
            assert_send_transaction_fail(
                node0,
                &tx,
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                Verification failed Transaction(DuplicateCellDeps(",
            );
        }

        {
            info!("test: two code-type cell deps have same data is allowed");
            let tx = TransactionBuilder::default()
                .cell_dep(dep1.clone())
                .cell_dep(dep2)
                .input(initial_inputs.next().unwrap())
                .output(empty_output.clone())
                .output_data(Default::default())
                .build();
            node0.submit_transaction(&tx);
            mine_until_bool(node0, || is_transaction_committed(node0, &tx));
        }

        {
            info!("test: hybrid types cell deps have same data is allowed");
            let tx = TransactionBuilder::default()
                .cell_dep(dep1)
                .cell_dep(dep3.clone())
                .input(initial_inputs.next().unwrap())
                .output(empty_output.clone())
                .output_data(Default::default())
                .build();
            node0.submit_transaction(&tx);
            mine_until_bool(node0, || is_transaction_committed(node0, &tx));
        }

        {
            info!("test: duplicate depgroup-type cell deps is not allowed.");
            let tx = TransactionBuilder::default()
                .cell_dep(dep3.clone())
                .cell_dep(dep3.clone())
                .input(initial_inputs.next().unwrap())
                .output(empty_output.clone())
                .output_data(Default::default())
                .build();
            assert_send_transaction_fail(
                node0,
                &tx,
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                Verification failed Transaction(DuplicateCellDeps(",
            );
        }

        {
            info!("test: two depgroup-type cell deps have same data is allowed");
            let tx = TransactionBuilder::default()
                .cell_dep(dep4.clone())
                .cell_dep(dep4b)
                .input(initial_inputs.next().unwrap())
                .output(empty_output.clone())
                .output_data(Default::default())
                .build();
            node0.submit_transaction(&tx);
            mine_until_bool(node0, || is_transaction_committed(node0, &tx));
        }

        {
            info!("test: two depgroup-type cell deps point to same data is allowed");
            let tx = TransactionBuilder::default()
                .cell_dep(dep3)
                .cell_dep(dep4)
                .input(initial_inputs.next().unwrap())
                .output(empty_output)
                .output_data(Default::default())
                .build();
            node0.submit_transaction(&tx);
            mine_until_bool(node0, || is_transaction_committed(node0, &tx));
        }
    }
}
