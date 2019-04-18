use crate::{sleep, Net, Spec};
use ckb_core::transaction::TransactionBuilder;
use log::info;

pub struct ValidSinceTxsInOneBlockSuccess;

impl Spec for ValidSinceTxsInOneBlockSuccess {
    fn run(&self, net: Net) {
        info!("Running ValidSinceTxsInOneBlockSuccess");
        let node0 = &net.nodes[0];

        info!("Generate 2 tx in same block");
        node0.generate_block();
        let tx_hash_0 = node0.generate_transaction();
        let tx = {
            let tx = node0.new_transaction(tx_hash_0.clone());
            let mut inputs = tx.inputs().to_owned();
            // restrict tx, tx should be mined after block number 4
            inputs[0].valid_since = 0x0000_0000_0000_0004;
            TransactionBuilder::default()
                .transaction(tx)
                .inputs_clear()
                .inputs(inputs)
                .build()
        };
        let tx_hash_1 = tx.hash().clone();
        node0
            .rpc_client()
            .send_transaction((&tx).into())
            .call()
            .expect("send tx");

        // mine 2 txs
        info!("Mine 2 tx");
        node0.generate_block();
        sleep(1);
        node0.generate_block();
        sleep(1);
        // mine one block, 2 txs should be commit
        info!("Waiting for mine");
        node0.generate_block();
        sleep(1);

        let tip_block = node0.get_tip_block();
        let commit_txs_hash: Vec<_> = tip_block
            .commit_transactions()
            .iter()
            .map(|tx| tx.hash().clone())
            .collect();

        info!("2 txs should included in commit_transactions");
        assert_eq!(tip_block.header().number(), 4);
        assert!(commit_txs_hash.contains(&tx_hash_0));
        assert!(commit_txs_hash.contains(&tx_hash_1));
    }

    fn num_nodes(&self) -> usize {
        1
    }
}
