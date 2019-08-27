use crate::{Net, Spec};
use ckb_types::prelude::Unpack;
use log::info;

pub struct TemplateSizeLimit;

impl Spec for TemplateSizeLimit {
    crate::name!("template_size_limit");

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();

        info!("Generate 6 txs");
        let mut txs_hash = Vec::new();
        let block = node.get_tip_block();
        let cellbase = &block.transactions()[0];
        let capacity = cellbase.outputs().get(0).unwrap().capacity().unpack();
        let tx = node.new_transaction_with_since_capacity(
            cellbase.hash().to_owned().unpack(),
            0,
            capacity,
        );
        let mut hash = node.rpc_client().send_transaction(tx.data().into());
        txs_hash.push(hash.clone());

        (0..5).for_each(|_| {
            let tx = node.new_transaction_with_since_capacity(hash.clone(), 0, capacity);
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        let _ = node.generate_block();
        let _ = node.generate_block(); // skip

        let new_block = node.new_block(None, None, None);
        assert_eq!(new_block.serialized_size(), 2430);
        assert_eq!(new_block.transactions().len(), 7);

        let new_block = node.new_block(Some(1000), None, None);
        assert_eq!(new_block.transactions().len(), 2);
    }
}
