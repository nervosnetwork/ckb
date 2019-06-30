use crate::{assert_regex_match, Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_chain_spec::ChainSpec;
use ckb_core::BlockNumber;
use log::info;

const MATURITY: BlockNumber = 5;

pub struct CellbaseMaturity;

impl Spec for CellbaseMaturity {
    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();

        info!("Use generated block's cellbase as tx input");
        let tip_block = node.get_tip_block();
        let tx = node.new_transaction(tip_block.transactions()[0].hash().to_owned());

        (0..MATURITY - DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|i| {
            info!("Tx is not maturity in N + {} block", i);
            let error = node.rpc_client().send_transaction((&tx).into());
            assert_regex_match(&error.to_string(), r"InvalidTx\(CellbaseImmaturity\)");
            node.generate_block();
        });

        info!(
            "Tx will be added to pending pool in N + {} block",
            MATURITY - DEFAULT_TX_PROPOSAL_WINDOW.0
        );
        let tx_hash = node.rpc_client().send_transaction((&tx).into());
        assert_eq!(tx_hash, tx.hash().to_owned());
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

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|spec_config| {
            spec_config.params.cellbase_maturity = MATURITY;
        })
    }
}
