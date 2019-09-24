use crate::{Net, Spec};
use ckb_types::prelude::Unpack;
use log::info;

pub struct TemplateSizeLimit;

impl Spec for TemplateSizeLimit {
    crate::name!("template_size_limit");

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 1 block");
        let blank_block = node.new_block(None, None, None);
        node.submit_block(&blank_block.data());
        let blank_block_size = blank_block.serialized_size();

        info!("Generate 6 txs");
        let mut txs_hash = Vec::new();
        let block = node.get_tip_block();
        let cellbase = &block.transactions()[0];
        let capacity = cellbase.outputs().get(0).unwrap().capacity().unpack();
        let tx = node.new_transaction_with_since_capacity(cellbase.hash(), 0, capacity);
        let tx_size = tx.serialized_size();
        info!(
            "blank_block_size: {}, tx_size: {}",
            blank_block_size, tx_size
        );

        let mut hash = node.rpc_client().send_transaction(tx.data().into());
        txs_hash.push(hash.clone());

        (0..5).for_each(|_| {
            let tx = node.new_transaction_with_since_capacity(hash.clone(), 0, capacity);
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        // skip proposal window
        node.generate_block();
        node.generate_block();

        let new_block = node.new_block(None, None, None);
        assert_eq!(new_block.serialized_size(), blank_block_size + tx_size * 6);
        // 6 txs + 1 cellbase tx
        assert_eq!(new_block.transactions().len(), 7);

        for bytes_limit in (1000..=2000).step_by(100) {
            let new_block = node.new_block(Some(bytes_limit), None, None);
            let tx_num = ((bytes_limit as usize) - blank_block_size) / tx_size;
            assert_eq!(new_block.transactions().len(), tx_num + 1);
        }
    }
}
