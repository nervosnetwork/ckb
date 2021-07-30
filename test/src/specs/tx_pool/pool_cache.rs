use crate::util::mining::{mine, mine_until_out_bootstrap_period};
use crate::{Node, Spec};
use ckb_logger::info;

pub struct PoolCache;

impl Spec for PoolCache {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];

        info!("Generate 1 block on node0");
        mine_until_out_bootstrap_period(node0);

        info!("Generate 6 txs on node0");
        let mut txs_hash1 = Vec::new();
        let mut txs_hash2 = Vec::new();
        let mut hash = node0.generate_transaction();
        txs_hash1.push(hash.clone());

        (0..5).for_each(|_| {
            let tx = node0.new_transaction(hash.clone());
            hash = node0.rpc_client().send_transaction(tx.data().into());
            txs_hash1.push(hash.clone());
        });

        info!("Generate 1 more blocks on node0");
        mine(node0, 1);

        info!("Generate 5 more txs on node0");
        (0..5).for_each(|_| {
            let tx = node0.new_transaction(hash.clone());
            hash = node0.rpc_client().send_transaction(tx.data().into());
            txs_hash2.push(hash.clone());
        });

        info!("Generate 1 more blocks on node0");
        mine(node0, 1);

        node0.wait_for_tx_pool();

        let tx_pool_info_original = node0.get_tip_tx_pool_info();

        info!("Stop node0 gracefully");
        node0.stop_gracefully();

        info!("Start node0");
        node0.start();

        let tx_pool_info_reloaded = node0.get_tip_tx_pool_info();
        info!("TxPool should be same as before");
        assert_eq!(tx_pool_info_original, tx_pool_info_reloaded);

        info!("Check the specific values of TxPool state");
        node0.assert_tx_pool_size(txs_hash2.len() as u64, txs_hash1.len() as u64);
        assert!(tx_pool_info_reloaded.total_tx_size.value() > 0);
        assert!(tx_pool_info_reloaded.total_tx_cycles.value() > 0);
        assert!(tx_pool_info_reloaded.last_txs_updated_at.value() > 0);
    }
}
