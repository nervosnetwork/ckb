use crate::utils::assert_send_transaction_fail;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};

use ckb_types::core::BlockNumber;
use log::info;

const MATURITY: BlockNumber = 5;

pub struct CellbaseMaturity;

impl Spec for CellbaseMaturity {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW.1 + 2 block");
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        info!("Use generated block's cellbase as tx input");
        let tip_block = node.get_tip_block();
        let tx = node.new_transaction(tip_block.transactions()[0].hash());

        (0..MATURITY - DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|i| {
            info!("Tx is not maturity in N + {} block", i);
            assert_send_transaction_fail(node, &tx, "CellbaseImmaturity");
            node.generate_block();
        });

        info!(
            "Tx will be added to pending pool in N + {} block",
            MATURITY - DEFAULT_TX_PROPOSAL_WINDOW.0
        );
        let tx_hash = node.rpc_client().send_transaction(tx.data().into());
        assert_eq!(tx_hash, tx.hash());
        node.assert_tx_pool_size(1, 0);

        info!(
            "Tx will be added to proposed pool in N + {} block",
            MATURITY
        );
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });

        node.assert_tx_pool_size(0, 1);
        node.generate_block();
        node.assert_tx_pool_size(0, 0);
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.cellbase_maturity = MATURITY;
    }
}
