use crate::util::mining::{mine, mine_until_out_bootstrap_period};
use crate::{Node, Spec};
use ckb_logger::info;

pub struct PoolPersisted;

impl Spec for PoolPersisted {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];

        info!("Generate 1 block on node0");
        mine_until_out_bootstrap_period(node0);

        info!("Generate 6 txs on node0");
        let mut hash = node0.generate_transaction();

        (0..5).for_each(|_| {
            let tx = node0.new_transaction(hash.clone());
            hash = node0.rpc_client().send_transaction(tx.data().into());
        });

        info!("Generate 1 more blocks on node0");
        mine(node0, 1);

        info!("Generate 5 more txs on node0");
        (0..5).for_each(|_| {
            let tx = node0.new_transaction(hash.clone());
            hash = node0.rpc_client().send_transaction(tx.data().into());
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
        info!("tx_pool_info_original: {:?}", tx_pool_info_original);
        info!("tx_pool_info_reloaded: {:?}", tx_pool_info_reloaded);
        assert_eq!(
            tx_pool_info_original.proposed,
            tx_pool_info_reloaded.proposed
        );
        assert_eq!(tx_pool_info_original.orphan, tx_pool_info_reloaded.orphan);
        assert_eq!(tx_pool_info_original.pending, tx_pool_info_reloaded.pending);
        assert_eq!(
            tx_pool_info_original.total_tx_size,
            tx_pool_info_reloaded.total_tx_size
        );
        assert_eq!(
            tx_pool_info_original.total_tx_cycles,
            tx_pool_info_reloaded.total_tx_cycles
        );
    }
}
