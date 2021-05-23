use crate::util::{
    check::{assert_epoch_should_be, assert_submit_block_fail, assert_submit_block_ok},
    mining::{mine, mine_until_epoch, mine_until_out_bootstrap_period},
};
use crate::utils::assert_send_transaction_fail;
use crate::{Node, Spec};
use ckb_logger::info;
use ckb_types::{
    core::{BlockView, TransactionView, Version},
    packed,
    prelude::*,
};

const GENESIS_EPOCH_LENGTH: u64 = 10;

const ERROR_BLOCK_VERSION: &str = "Invalid: Header(Version(BlockVersionError(";
const ERROR_TX_VERSION: &str =
    "TransactionFailedToVerify: Verification failed Transaction(MismatchedVersion";

pub struct CheckBlockVersion;
pub struct CheckTxVersion;

impl Spec for CheckBlockVersion {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;

        mine_until_out_bootstrap_period(node);

        assert_epoch_should_be(node, 1, 2, epoch_length);
        {
            info!("CKB v2019, submit block with version 1 is failed");
            let block = create_block_with_version(node, 1);
            assert_submit_block_fail(node, &block, ERROR_BLOCK_VERSION);
        }
        assert_epoch_should_be(node, 1, 2, epoch_length);
        {
            info!("CKB v2019, submit block with version 0 is passed");
            let block = create_block_with_version(node, 0);
            assert_submit_block_ok(node, &block);
        }
        assert_epoch_should_be(node, 1, 3, epoch_length);
        mine_until_epoch(node, 1, epoch_length - 2, epoch_length);
        {
            info!("CKB v2019, submit block with version 1 is failed (boundary)");
            let block = create_block_with_version(node, 1);
            assert_submit_block_fail(node, &block, ERROR_BLOCK_VERSION);
        }
        assert_epoch_should_be(node, 1, epoch_length - 2, epoch_length);
        {
            info!("CKB v2019, submit block with version 0 is passed (boundary)");
            let block = create_block_with_version(node, 0);
            assert_submit_block_ok(node, &block);
        }
        assert_epoch_should_be(node, 1, epoch_length - 1, epoch_length);
        {
            info!("CKB v2021, submit block with version 1 is passed (boundary)");
            let block = create_block_with_version(node, 1);
            assert_submit_block_ok(node, &block);
        }
        assert_epoch_should_be(node, 2, 0, epoch_length);
        {
            info!("CKB v2021, submit block with version 0 is passed (boundary)");
            let block = create_block_with_version(node, 0);
            assert_submit_block_ok(node, &block);
        }
        assert_epoch_should_be(node, 2, 1, epoch_length);
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_pr_0230 = Some(2);
        }
    }
}

impl Spec for CheckTxVersion {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;

        mine_until_out_bootstrap_period(node);

        assert_epoch_should_be(node, 1, 2, epoch_length);
        {
            let input_cell_hash = &node.get_tip_block().transactions()[0].hash();

            info!("CKB v2019, submit transaction with version 1 is failed");
            let tx = create_transaction_with_version(node, input_cell_hash.clone(), 0, 1);
            assert_send_transaction_fail(node, &tx, ERROR_TX_VERSION);

            info!("CKB v2019, submit block with version 0 is passed");
            let tx = create_transaction_with_version(node, input_cell_hash.clone(), 0, 0);
            let res = node.rpc_client().send_transaction_result(tx.data().into());
            assert!(res.is_ok(), "result: {:?}", res.unwrap_err());
        }
        mine_until_epoch(node, 1, epoch_length - 4, epoch_length);
        {
            let input_cell_hash = &node.get_tip_block().transactions()[0].hash();

            info!("CKB v2019, submit transaction with version 1 is failed (boundary)");
            let tx = create_transaction_with_version(node, input_cell_hash.clone(), 0, 1);
            assert_send_transaction_fail(node, &tx, ERROR_TX_VERSION);

            info!("CKB v2019, submit block with version 0 is passed (boundary)");
            let tx = create_transaction_with_version(node, input_cell_hash.clone(), 0, 0);
            let res = node.rpc_client().send_transaction_result(tx.data().into());
            assert!(res.is_ok(), "result: {:?}", res.unwrap_err());
        }
        mine(node, 1);
        assert_epoch_should_be(node, 1, epoch_length - 3, epoch_length);
        {
            let input_cell_hash = &node.get_tip_block().transactions()[0].hash();
            info!("CKB v2021, submit transaction with version 1 is passed (boundary)");
            let tx = create_transaction_with_version(node, input_cell_hash.clone(), 0, 1);
            let res = node.rpc_client().send_transaction_result(tx.data().into());
            assert!(res.is_ok(), "result: {:?}", res.unwrap_err());

            let input_cell_hash = &tx.hash();
            info!("CKB v2021, submit block with version 0 is passed (boundary)");
            let tx = create_transaction_with_version(node, input_cell_hash.clone(), 0, 0);
            let res = node.rpc_client().send_transaction_result(tx.data().into());
            assert!(res.is_ok(), "result: {:?}", res.unwrap_err());

            let input_cell_hash = &tx.hash();
            info!("CKB v2021, submit transaction with version 100 is passed (boundary)");
            let tx = create_transaction_with_version(node, input_cell_hash.clone(), 0, 100);
            let res = node.rpc_client().send_transaction_result(tx.data().into());
            assert!(res.is_ok(), "result: {:?}", res.unwrap_err());
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_pr_0230 = Some(2);
        }
    }
}

fn create_block_with_version(node: &Node, version: Version) -> BlockView {
    node.new_block_builder(None, None, None)
        .version(version.pack())
        .build()
}

fn create_transaction_with_version(
    node: &Node,
    hash: packed::Byte32,
    index: u32,
    version: Version,
) -> TransactionView {
    let always_success_cell_dep = node.always_success_cell_dep();
    let always_success_script = node.always_success_script();

    let input_cell = node
        .rpc_client()
        .get_transaction(hash.clone())
        .unwrap()
        .transaction
        .inner
        .outputs[index as usize]
        .to_owned();

    let cell_input = packed::CellInput::new(packed::OutPoint::new(hash, index), 0);
    let cell_output = packed::CellOutput::new_builder()
        .capacity((input_cell.capacity.value() - 1).pack())
        .lock(always_success_script)
        .build();

    TransactionView::new_advanced_builder()
        .version(version.pack())
        .cell_dep(always_success_cell_dep)
        .input(cell_input)
        .output(cell_output)
        .output_data(Default::default())
        .build()
}
