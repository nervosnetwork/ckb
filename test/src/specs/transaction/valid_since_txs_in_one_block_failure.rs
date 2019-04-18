use crate::{Net, Spec};
use ckb_core::transaction::TransactionBuilder;
use log::info;

pub struct ValidSinceTxsInOneBlockFailure;

impl Spec for ValidSinceTxsInOneBlockFailure {
    fn run(&self, net: Net) {
        info!("Running ValidSinceTxsInOneBlockFailure");
        let node0 = &net.nodes[0];

        info!("Generate 2 tx in same block, tx2 with a immature valid_since input");
        node0.generate_block();
        let tx_hash_0 = node0.generate_transaction();
        let tx = {
            let tx = node0.new_transaction(tx_hash_0.clone());
            let mut inputs = tx.inputs().to_owned();
            // restrict tx, tx should be mined after 1 blocks from tx1 to be mined
            inputs[0].valid_since = 0x8000_0000_0000_0001;
            TransactionBuilder::default()
                .transaction(tx)
                .inputs_clear()
                .inputs(inputs)
                .build()
        };
        assert!(node0
            .rpc_client()
            .send_transaction((&tx).into())
            .call()
            .is_err());
    }

    fn num_nodes(&self) -> usize {
        1
    }
}
