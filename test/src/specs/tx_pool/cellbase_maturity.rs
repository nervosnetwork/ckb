use crate::{assert_regex_match, Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_core::BlockNumber;
use log::info;

const MATURITY: BlockNumber = 5;

pub struct CellbaseMaturity;

impl Spec for CellbaseMaturity {
    fn run(&self, net: Net) {
        info!("Running CellbaseMaturity");
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();

        info!("Use generated block's cellbase as tx input");
        let tip_block = node.get_tip_block();
        let tx = node.new_transaction(tip_block.transactions()[0].hash().to_owned());

        (0..MATURITY - DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|i| {
            info!("Tx is not maturity in N + {} block", i);
            let error = node
                .rpc_client()
                .send_transaction((&tx).into())
                .call()
                .unwrap_err();
            assert_regex_match(&error.to_string(), r"InvalidTx\(CellbaseImmaturity\)");
            node.generate_block();
        });

        info!(
            "Tx will be added to pending pool in N + {} block",
            MATURITY - DEFAULT_TX_PROPOSAL_WINDOW.0
        );
        let tx_hash = node
            .rpc_client()
            .send_transaction((&tx).into())
            .call()
            .unwrap();
        assert_eq!(tx_hash, tx.hash().to_owned());
        let tx_pool_info = node
            .rpc_client()
            .tx_pool_info()
            .call()
            .expect("rpc call tx_pool_info failed");
        assert_eq!(tx_pool_info.pending, 1);

        info!("Tx will be added to staging pool in N + {} block", MATURITY);
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|_| {
            node.generate_block();
        });
        let tx_pool_info = node
            .rpc_client()
            .tx_pool_info()
            .call()
            .expect("rpc call tx_pool_info failed");
        assert_eq!(tx_pool_info.staging, 1);

        node.generate_block();
        let tx_pool_info = node
            .rpc_client()
            .tx_pool_info()
            .call()
            .expect("rpc call tx_pool_info failed");
        assert_eq!(tx_pool_info.staging, 0);
    }

    fn num_nodes(&self) -> usize {
        1
    }

    fn cellbase_maturity(&self) -> Option<BlockNumber> {
        Some(MATURITY as BlockNumber)
    }
}
