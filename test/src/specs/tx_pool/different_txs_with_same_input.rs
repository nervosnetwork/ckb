use crate::{Net, Spec};
use ckb_core::transaction::{Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, Capacity};
use log::info;

pub struct DifferentTxsWithSameInput;

impl Spec for DifferentTxsWithSameInput {
    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];

        node0.generate_block();
        let tx_hash_0 = node0.generate_transaction();
        info!("Generate 2 txs with same input");
        let tx1 = node0.new_transaction(tx_hash_0.clone());
        let tx2_temp = node0.new_transaction(tx_hash_0.clone());
        // Set tx2 fee to a higher value
        let mut output = tx2_temp.outputs()[0].clone();
        output.capacity = capacity_bytes!(40_000);
        let tx2 = TransactionBuilder::from_transaction(tx2_temp)
            .outputs_clear()
            .output(output)
            .build();
        node0.rpc_client().send_transaction((&tx1).into());
        node0.rpc_client().send_transaction((&tx1).into());

        node0.generate_block();
        node0.generate_block();

        info!("RBF (Replace-By-Fees) is not implemented");
        info!("Tx1 should be included in the next + 2 block");
        node0.generate_block();
        let tip_block = node0.get_tip_block();
        let commit_txs_hash: Vec<_> = tip_block
            .transactions()
            .iter()
            .map(Transaction::hash)
            .collect();

        assert!(commit_txs_hash.contains(&tx1.hash()));
        assert!(!commit_txs_hash.contains(&tx2.hash()));
    }
}
