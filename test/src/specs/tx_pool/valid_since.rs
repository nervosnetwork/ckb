use crate::{assert_regex_match, Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_core::BlockNumber;
use log::info;

pub struct ValidSince;

#[allow(clippy::identity_op)]
impl Spec for ValidSince {
    fn run(&self, net: Net) {
        info!("Running ValidSince");
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();

        // test relative block number since
        info!("Use tip block cellbase as tx input with a relative block number since");
        let relative_blocks: BlockNumber = 5;
        let since = (0b1000_0000 << 56) + relative_blocks;
        let tip_block = node.get_tip_block();
        let tx =
            node.new_transaction_with_since(tip_block.transactions()[0].hash().to_owned(), since);

        (0..relative_blocks - DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|i| {
            info!("Tx is immature in block N + {}", i);
            let error = node
                .rpc_client()
                .send_transaction((&tx).into())
                .call()
                .unwrap_err();
            assert_regex_match(&error.to_string(), r"InvalidTx\(Immature\)");
            node.generate_block();
        });

        info!(
            "Tx will be added to pending pool in N + {} block",
            relative_blocks - DEFAULT_TX_PROPOSAL_WINDOW.0
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

        info!(
            "Tx will be added to staging pool in N + {} block",
            relative_blocks
        );
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

        // test absolute block number since
        let tip_number: BlockNumber = node
            .rpc_client()
            .get_tip_block_number()
            .call()
            .unwrap()
            .parse()
            .unwrap();
        info!(
            "Use tip block {} cellbase as tx input with an absolute block number since",
            tip_number
        );
        let absolute_block: BlockNumber = 10;
        let since = (0b0000_0000 << 56) + absolute_block;
        let tip_block = node.get_tip_block();
        let tx =
            node.new_transaction_with_since(tip_block.transactions()[0].hash().to_owned(), since);

        (tip_number..absolute_block - DEFAULT_TX_PROPOSAL_WINDOW.0).for_each(|i| {
            info!("Tx is immature in block {}", i);
            let error = node
                .rpc_client()
                .send_transaction((&tx).into())
                .call()
                .unwrap_err();
            assert_regex_match(&error.to_string(), r"InvalidTx\(Immature\)");
            node.generate_block();
        });

        info!(
            "Tx will be added to pending pool in {} block",
            absolute_block - DEFAULT_TX_PROPOSAL_WINDOW.0
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

        info!(
            "Tx will be added to staging pool in {} block",
            absolute_block
        );
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
}
