use crate::{Net, Spec};
use ckb_core::block::Block;
use ckb_core::transaction::ProposalShortId;
use log::info;

pub struct MiningBasic {}

impl Spec for MiningBasic {
    fn run(&self, net: &Net) {
        info!("Running MiningBasic");
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();

        info!("Use generated block's cellbase as tx input");
        let transaction_hash = node.generate_transaction();
        let block1_hash = node.generate_block();
        let block2_hash = node.generate_block();

        let block1: Block = node
            .rpc_client()
            .get_block(block1_hash)
            .call()
            .unwrap()
            .unwrap()
            .into();
        let block2: Block = node
            .rpc_client()
            .get_block(block2_hash)
            .call()
            .unwrap()
            .unwrap()
            .into();

        info!("Generated tx should be included in next block's proposal txs");
        assert!(block1
            .proposal_transactions()
            .iter()
            .any(|id| ProposalShortId::from_h256(&transaction_hash).eq(id)));

        info!("Generated tx should be included in next+1 block's commit txs");
        assert!(block2
            .commit_transactions()
            .iter()
            .any(|tx| transaction_hash.eq(&tx.hash())));
    }

    fn num_nodes(&self) -> usize {
        1
    }
}
