use crate::{assert_regex_match, utils::is_committed, Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_chain_spec::ChainSpec;
use ckb_types::core::BlockNumber;
use log::info;

const MATURITY: BlockNumber = 5;

pub struct ReferenceHeaderMaturity;

impl Spec for ReferenceHeaderMaturity {
    crate::name!("reference_header_maturity");

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();
        info!("Use generated block's cellbase as tx input");
        let base_block = node.get_tip_block();
        info!("Ensure cellbase is matured");
        node.generate_blocks(5);

        info!("Reference tip block's header to test for maturity");
        let tip_block = node.get_tip_block();

        let tx = node.new_transaction(base_block.transactions()[0].hash());
        let tx = tx
            .data()
            .as_advanced_builder()
            .header_dep(tip_block.hash())
            .build();

        (0..MATURITY).for_each(|i| {
            info!("Tx is not matured in N + {} block", i);
            let error = node
                .rpc_client()
                .send_transaction_result(tx.clone().data().into())
                .unwrap_err();
            assert_regex_match(&error.to_string(), r"ImmatureHeader");
            node.generate_block();
        });

        info!("Tx will be added to pending pool in N + {} block", MATURITY,);
        let tx_hash = node.rpc_client().send_transaction(tx.clone().data().into());
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

        info!("Tx will be eventually accepted on chain",);
        node.generate_blocks(5);
        let tx_status = node
            .rpc_client()
            .get_transaction(tx.hash())
            .expect("get sent transaction");
        assert!(
            is_committed(&tx_status),
            "ensure_committed failed {}",
            tx.hash(),
        );
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|spec_config| {
            spec_config.params.cellbase_maturity = MATURITY;
        })
    }
}
