use crate::{Net, Spec};
use log::info;

pub struct TemplateSizeLimit;

impl Spec for TemplateSizeLimit {
    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();

        info!("Generate 6 txs");
        let mut txs_hash = Vec::new();
        let mut hash = node.generate_transaction();
        txs_hash.push(hash.clone());

        (0..5).for_each(|_| {
            let tx = node.new_transaction(hash.clone());
            hash = node.rpc_client().send_transaction((&tx).into());
            txs_hash.push(hash.clone());
        });

        let _ = node.generate_block();
        let _ = node.generate_block(); // skip

        let new_block = node.new_block(None, None, None);
        assert_eq!(new_block.serialized_size(0), 1240);
        assert_eq!(new_block.transactions().len(), 7);

        let new_block = node.new_block(Some(1000), None, None);
        assert_eq!(new_block.transactions().len(), 5);
    }
}
