use crate::{
    util::{
        check::{assert_epoch_should_less_than, is_transaction_committed},
        mining::{mine, mine_until_bool, mine_until_epoch, mine_until_out_bootstrap_period},
    },
    utils::assert_send_transaction_fail,
    Node, Spec,
};
use ckb_logger::{debug, info};
use ckb_types::{
    core::{Capacity, TransactionView},
    packed,
    prelude::*,
};
use std::fmt;

const CELLBASE_MATURITY: u64 = 2;
const GENESIS_EPOCH_LENGTH: u64 = 5;
const CKB2021_START_EPOCH: u64 = 10;

const INITIAL_INPUTS_COUNT: usize = 1 + 1 + 1;

pub struct ImmatureHeaderDeps;

#[derive(Debug, Clone, Copy)]
enum ExpectedResult {
    ShouldBePassed,
    ImmatureHeader,
}

struct ImmatureHeaderDepsTestRunner<'a> {
    node: &'a Node,
}

impl Spec for ImmatureHeaderDeps {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;
        let maturity_blocks = CELLBASE_MATURITY * GENESIS_EPOCH_LENGTH;

        mine_until_out_bootstrap_period(node);

        info!("Create enough input cells for tests.");
        let runner = ImmatureHeaderDepsTestRunner::new(node);
        let mut inputs = runner
            .mine_cellbases_as_inputs(INITIAL_INPUTS_COUNT as u64)
            .into_iter();

        info!("Wait to let the input cells to be mature.");
        mine(node, maturity_blocks);

        {
            let tx = runner.create_tx_with_tip_header_as_dep(&mut inputs);
            {
                let res = ExpectedResult::ImmatureHeader;
                info!(
                    "CKB v2019: send tx with tip header as header dep is {}",
                    res
                );
                runner.test_send(&tx, res);
            }
            mine(node, maturity_blocks - 1);
            {
                let res = ExpectedResult::ImmatureHeader;
                info!(
                    "CKB v2019: send tx with immature header as header dep is {}",
                    res
                );
                runner.test_send(&tx, res);
            }
            mine(node, 1);
            {
                let res = ExpectedResult::ShouldBePassed;
                info!(
                    "CKB v2019: send tx with mature header as header dep is {}",
                    res
                );
                runner.test_send(&tx, res);
            }
        }
        assert_epoch_should_less_than(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine_until_epoch(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        {
            let tx = runner.create_tx_with_tip_header_as_dep(&mut inputs);
            {
                let res = ExpectedResult::ImmatureHeader;
                info!(
                    "CKB v2019 (boundary): send tx with tip header as header dep is {}",
                    res
                );
                runner.test_send(&tx, res);
            }
            mine(node, 1);
            {
                let res = ExpectedResult::ShouldBePassed;
                info!(
                    "CKB v2021 (boundary): send tx with tip header as header dep is {}",
                    res
                );
                runner.test_send(&tx, res);
            }
        }
        mine_until_epoch(node, ckb2019_last_epoch + 2, 0, epoch_length);
        {
            let tx = runner.create_tx_with_tip_header_as_dep(&mut inputs);
            let res = ExpectedResult::ShouldBePassed;
            info!(
                "CKB v2021: send tx with tip header as header dep is {}",
                res
            );
            runner.test_send(&tx, res);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.cellbase_maturity = Some(CELLBASE_MATURITY);
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if spec.params.hardfork.is_none() {
            spec.params.hardfork = Some(Default::default());
        }
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_0036 = Some(CKB2021_START_EPOCH);
        }
    }
}

impl fmt::Display for ExpectedResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ShouldBePassed => write!(f, "allowed"),
            _ => write!(f, "not allowed"),
        }
    }
}

impl ExpectedResult {
    fn error_message(self) -> Option<&'static str> {
        match self {
            Self::ShouldBePassed => None,
            Self::ImmatureHeader => Some(
                "{\"code\":-301,\"message\":\"TransactionFailedToResolve: \
                Resolve failed ImmatureHeader(Byte32(0x",
            ),
        }
    }
}

impl<'a> ImmatureHeaderDepsTestRunner<'a> {
    fn new(node: &'a Node) -> Self {
        Self { node }
    }

    fn mine_cellbases_as_inputs(&self, count: u64) -> Vec<packed::CellInput> {
        let start_block_number = self.node.get_tip_block_number() + 1;
        mine(self.node, count);
        (0..count)
            .into_iter()
            .map(|i| {
                let cellbase = self
                    .node
                    .get_block_by_number(start_block_number + i)
                    .transaction(0)
                    .expect("cellbase exists");
                let out_point = packed::OutPoint::new(cellbase.hash(), 0);
                packed::CellInput::new(out_point, 0)
            })
            .collect()
    }

    fn create_tx_with_tip_header_as_dep(
        &self,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
    ) -> TransactionView {
        let input = inputs.next().unwrap();
        let output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::shannons(0))
            .unwrap();
        let tip_block = self.node.get_tip_block();
        TransactionView::new_advanced_builder()
            .header_dep(tip_block.hash())
            .cell_dep(self.node.always_success_cell_dep())
            .input(input)
            .output(output)
            .output_data(Default::default())
            .build()
    }

    fn test_send(&self, tx: &TransactionView, expected: ExpectedResult) {
        if let Some(errmsg) = expected.error_message() {
            assert_send_transaction_fail(self.node, tx, &errmsg);
        } else {
            self.submit_transaction_until_committed(tx);
        }
    }

    fn submit_transaction_until_committed(&self, tx: &TransactionView) {
        debug!(">>> submit: transaction {:#x}.", tx.hash());
        self.node.submit_transaction(tx);
        mine_until_bool(self.node, || is_transaction_committed(self.node, tx));
    }
}
