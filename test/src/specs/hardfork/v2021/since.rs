use crate::util::check::{self, assert_epoch_should_be};
use crate::utils::{
    assert_send_transaction_fail, assert_send_transaction_ok, since_from_absolute_epoch_number,
    since_from_relative_epoch_number,
};
use crate::{Node, Spec};

use ckb_logger::info;
use ckb_types::core::{EpochNumberWithFraction, TransactionView};

const GENESIS_EPOCH_LENGTH: u64 = 10;

const ERROR_IMMATURE: &str = "TransactionFailedToVerify: Verification failed Transaction(Immature(";
const ERROR_INVALID_SINCE: &str =
    "TransactionFailedToVerify: Verification failed Transaction(InvalidSince(";

pub struct CheckAbsoluteEpochSince;
pub struct CheckRelativeEpochSince;

impl Spec for CheckAbsoluteEpochSince {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;

        node.mine_until_out_bootstrap_period();

        assert_epoch_should_be(node, 1, 2, epoch_length);
        {
            info!("CKB v2019, since absolute epoch failed");
            let tx = create_tx_since_absolute_epoch(node, 1, 3);
            assert_send_transaction_fail(node, &tx, ERROR_IMMATURE);
        }
        {
            info!("CKB v2019, since absolute epoch ok");
            let tx = create_tx_since_absolute_epoch(node, 1, 2);
            assert_send_transaction_ok(node, &tx);
        }
        {
            info!("CKB v2019, since absolute epoch ok (index=length=0)");
            let tx = create_tx_since_absolute_epoch_with_length(node, 1, 0, 0);
            assert_send_transaction_ok(node, &tx);
        }
        {
            info!("CKB v2019, since absolute epoch ok (index>length=0)");
            let tx = create_tx_since_absolute_epoch_with_length(node, 1, 1, 0);
            assert_send_transaction_ok(node, &tx);
        }
        node.mine_until_epoch(1, epoch_length - 2, epoch_length);
        {
            info!("CKB v2019, since absolute epoch failed (boundary)");
            let tx = create_tx_since_absolute_epoch(node, 1, epoch_length - 1);
            assert_send_transaction_fail(node, &tx, ERROR_IMMATURE);
        }
        {
            info!("CKB v2019, since absolute epoch ok (boundary)");
            let tx = create_tx_since_absolute_epoch(node, 1, epoch_length - 2);
            assert_send_transaction_ok(node, &tx);
        }
        node.mine(1);
        {
            info!("CKB v2019, since absolute epoch failed (boundary, malformed)");
            let tx = create_tx_since_absolute_epoch(node, 0, (epoch_length - 1) + epoch_length);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        node.mine(1);
        assert_epoch_should_be(node, 2, 0, epoch_length);
        {
            info!("CKB v2021, since absolute epoch failed (boundary, malformed)");
            let tx = create_tx_since_absolute_epoch(node, 0, epoch_length * 2);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            info!("CKB v2021, since absolute epoch failed (boundary, malformed)");
            let tx = create_tx_since_absolute_epoch(node, 1, epoch_length);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            info!("CKB v2021, since absolute epoch failed (boundary, index>length=0)");
            let tx = create_tx_since_absolute_epoch_with_length(node, 2, 1, 0);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            info!("CKB v2021, since absolute epoch ok (boundary, index=length=0)");
            let tx = create_tx_since_absolute_epoch_with_length(node, 2, 0, 0);
            assert_send_transaction_ok(node, &tx);
        }
        node.mine_until_epoch(3, 0, epoch_length);
        {
            info!("CKB v2021, since absolute epoch failed (malformed)");
            let tx = create_tx_since_absolute_epoch(node, 0, epoch_length * 3);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            info!("CKB v2021, since absolute epoch failed (malformed)");
            let tx = create_tx_since_absolute_epoch(node, 1, epoch_length * 2);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            info!("CKB v2021, since absolute epoch failed (malformed)");
            let tx = create_tx_since_absolute_epoch(node, 2, epoch_length);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            info!("CKB v2021, since absolute epoch failed (index>length=0)");
            let tx = create_tx_since_absolute_epoch_with_length(node, 3, 1, 0);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            info!("CKB v2021, since absolute epoch failed (index=length=0)");
            let tx = create_tx_since_absolute_epoch_with_length(node, 3, 0, 0);
            assert_send_transaction_ok(node, &tx);
        }
        node.mine(1);
        {
            info!("CKB v2021, since absolute epoch ok");
            let tx = create_tx_since_absolute_epoch(node, 3, 1);
            assert_send_transaction_ok(node, &tx);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if spec.params.hardfork.is_none() {
            spec.params.hardfork = Some(Default::default());
        }
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_0030 = Some(2);
        }
    }
}

impl Spec for CheckRelativeEpochSince {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;

        node.mine_until_out_bootstrap_period();

        assert_epoch_should_be(node, 1, 2, epoch_length);
        node.mine_until_epoch(1, epoch_length - 4, epoch_length);
        {
            info!("CKB v2019, since relative epoch failed");
            let tx = create_tx_since_relative_epoch(node, 1, 0);
            node.mine(epoch_length - 1);
            assert_send_transaction_fail(node, &tx, ERROR_IMMATURE);
            node.mine(1);
            info!("CKB v2019, since relative epoch ok");
            assert_send_transaction_ok(node, &tx);
        }
        assert_epoch_should_be(node, 2, epoch_length - 4, epoch_length);
        {
            info!("CKB v2019, since relative epoch failed (malformed)");
            let tx = create_tx_since_relative_epoch(node, 0, epoch_length);
            node.mine(epoch_length - 1);
            assert_send_transaction_fail(node, &tx, ERROR_IMMATURE);
            node.mine(1);
            info!("CKB v2019, since relative epoch ok (malformed)");
            assert_send_transaction_ok(node, &tx);
        }
        assert_epoch_should_be(node, 3, epoch_length - 4, epoch_length);
        {
            info!("CKB v2019, since relative epoch ok (index=length=0)");
            let tx = create_tx_since_relative_epoch_with_length(node, 1, 0, 0);
            node.mine(epoch_length);
            assert_send_transaction_ok(node, &tx);
        }
        {
            info!("CKB v2019, since relative epoch ok (index>length=0)");
            let tx = create_tx_since_relative_epoch_with_length(node, 1, 1, 0);
            node.mine(epoch_length);
            assert_send_transaction_ok(node, &tx);
        }
        assert_epoch_should_be(node, 5, epoch_length - 4, epoch_length);
        {
            let tx1 = create_tx_since_relative_epoch(node, 0, epoch_length);
            node.mine(1);
            let tx2 = create_tx_since_relative_epoch(node, 0, epoch_length);
            let tx3 = create_tx_since_relative_epoch_with_length(node, 1, 1, 0);
            node.mine(epoch_length - 2);

            info!("CKB v2019, since relative epoch failed (boundary, malformed)");
            assert_send_transaction_fail(node, &tx1, ERROR_IMMATURE);
            node.mine(1);
            info!("CKB v2019, since relative epoch ok (boundary, malformed)");
            assert_send_transaction_ok(node, &tx1);

            info!("CKB v2019, since relative epoch failed (boundary, malformed)");
            assert_send_transaction_fail(node, &tx2, ERROR_IMMATURE);

            info!("CKB v2019, since relative epoch failed (boundary, index>length=0)");
            assert_send_transaction_fail(node, &tx3, ERROR_IMMATURE);

            let proposed = node.mine_with_blocking(|template| template.proposals.is_empty());

            info!("CKB v2019, since relative epoch failed (boundary, malformed)");
            assert_send_transaction_fail(node, &tx2, ERROR_INVALID_SINCE);

            info!("CKB v2019, since relative epoch failed (boundary, index>length=0)");
            assert_send_transaction_fail(node, &tx3, ERROR_INVALID_SINCE);

            info!("CKB v2019, since relative epoch transaction will be committed (boundary, malformed)");
            assert_epoch_should_be(node, 6, epoch_length - 3, epoch_length);
            assert!(check::is_transaction_pending(node, &tx1));
            node.mine_with_blocking(|template| template.number.value() != (proposed + 1));
            assert!(check::is_transaction_proposed(node, &tx1));
            node.mine_with_blocking(|template| template.transactions.is_empty());
            assert!(check::is_transaction_committed(node, &tx1));
            assert_epoch_should_be(node, 6, epoch_length - 1, epoch_length);
        }
        {
            info!("CKB v2021, since relative epoch failed (malformed)");
            let tx = create_tx_since_relative_epoch(node, 0, epoch_length);
            node.mine(epoch_length - 1);
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
            node.mine(1);
            info!("CKB v2021, since relative epoch failed (malformed)");
            assert_send_transaction_fail(node, &tx, ERROR_INVALID_SINCE);
        }
        {
            let tx1 = create_tx_since_relative_epoch_with_length(node, 1, 1, 0);
            let tx2 = create_tx_since_relative_epoch_with_length(node, 1, 0, 0);

            node.mine(epoch_length);

            info!("CKB v2021, since relative epoch failed (index>length=0)");
            assert_send_transaction_fail(node, &tx1, ERROR_INVALID_SINCE);

            info!("CKB v2021, since relative epoch ok (index=length=0)");
            assert_send_transaction_ok(node, &tx2);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if spec.params.hardfork.is_none() {
            spec.params.hardfork = Some(Default::default());
        }
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_0030 = Some(7);
        }
    }
}

fn create_tx_since_absolute_epoch_with_length(
    node: &Node,
    number: u64,
    index: u64,
    length: u64,
) -> TransactionView {
    let epoch = EpochNumberWithFraction::new_unchecked(number, index, length);
    let since = since_from_absolute_epoch_number(epoch);
    let cellbase = node.get_tip_block().transactions()[0].clone();
    node.new_transaction_with_since(cellbase.hash(), since)
}

fn create_tx_since_relative_epoch_with_length(
    node: &Node,
    number: u64,
    index: u64,
    length: u64,
) -> TransactionView {
    let epoch = EpochNumberWithFraction::new_unchecked(number, index, length);
    let since = since_from_relative_epoch_number(epoch);
    let cellbase = node.get_tip_block().transactions()[0].clone();
    node.new_transaction_with_since(cellbase.hash(), since)
}

fn create_tx_since_absolute_epoch(node: &Node, number: u64, index: u64) -> TransactionView {
    create_tx_since_absolute_epoch_with_length(node, number, index, GENESIS_EPOCH_LENGTH)
}

fn create_tx_since_relative_epoch(node: &Node, number: u64, index: u64) -> TransactionView {
    create_tx_since_relative_epoch_with_length(node, number, index, GENESIS_EPOCH_LENGTH)
}
