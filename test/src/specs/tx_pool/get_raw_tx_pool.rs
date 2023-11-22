use crate::{Node, Spec};
use ckb_jsonrpc_types::RawTxPool;
use ckb_logger::info;
use ckb_types::{prelude::Unpack, H256};

pub struct GetRawTxPool;

impl Spec for GetRawTxPool {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &mut nodes[0];

        node0.mine_until_out_bootstrap_period();

        info!("Generate 6 txs on node0");
        let mut txs_hash = vec![node0.generate_transaction()];

        (0..5).for_each(|_| {
            let tx = node0.new_transaction(txs_hash.last().unwrap().clone());
            txs_hash.push(node0.rpc_client().send_transaction(tx.data().into()));
        });

        let mut pending: Vec<H256> = txs_hash.iter().map(Unpack::unpack).collect();
        pending.sort();
        let result = node0.rpc_client().get_raw_tx_pool(None);
        match result {
            RawTxPool::Ids(ids) => {
                assert_eq!(0, ids.proposed.len());
                let mut ids = ids.pending;
                ids.sort();
                assert_eq!(ids, pending);
            }
            _ => panic!("get_raw_tx_pool(true) should return entries"),
        }
        match node0.rpc_client().get_raw_tx_pool(Some(true)) {
            RawTxPool::Ids(_ids) => {
                panic!("get_raw_tx_pool(true) should return entries");
            }
            RawTxPool::Verbose(entries) => {
                assert_eq!(6, entries.pending.len());
            }
        }
    }
}
