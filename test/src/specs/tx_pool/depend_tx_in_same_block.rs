use crate::{sleep, Net, Spec};
use log::info;

pub struct DepentTxInSameBlock;

impl Spec for DepentTxInSameBlock {
    fn run(&self, net: Net) {
        info!("Running DepentTxInSameBlock");
        let node0 = &net.nodes[0];

        info!("Generate 2 tx in same block");
        node0.generate_block();
        let tx_hash_0 = node0.generate_transaction();
        let tx = node0.new_transaction(tx_hash_0.clone());
        let tx_hash_1 = tx.hash().clone();
        node0.rpc_client().send_transaction((&tx).into());

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
            .transactions()
            .iter()
            .map(|tx| tx.hash().clone())
            .collect();

        info!("2 txs should included in commit_transactions");
        assert!(commit_txs_hash.contains(&tx_hash_0));
        assert!(commit_txs_hash.contains(&tx_hash_1));
    }

    fn num_nodes(&self) -> usize {
        1
    }
}
