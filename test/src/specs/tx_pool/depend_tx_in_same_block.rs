use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::{core::TransactionView, packed::ProposalShortId};
use log::info;

pub struct DepentTxInSameBlock;

impl Spec for DepentTxInSameBlock {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        info!("Generate 2 tx in same block");
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let tx_hash_0 = node0.generate_transaction();
        let tx = node0.new_transaction(tx_hash_0.clone());
        let tx_hash_1 = tx.hash();
        node0.rpc_client().send_transaction(tx.data().into());

        info!("Generated 2 tx should be included in the next block's proposals");
        node0.generate_block();
        let proposal_block = node0.get_tip_block();
        let proposal_ids: Vec<_> = proposal_block.union_proposal_ids_iter().collect();
        assert!(proposal_ids.contains(&ProposalShortId::from_tx_hash(&tx_hash_0)));
        assert!(proposal_ids.contains(&ProposalShortId::from_tx_hash(&tx_hash_1)));

        node0.generate_block();

        info!("Generated 2 tx should be included in the next + 2 block");
        node0.generate_block();
        let tip_block = node0.get_tip_block();
        let commit_txs_hash: Vec<_> = tip_block
            .transactions()
            .iter()
            .map(TransactionView::hash)
            .collect();

        assert!(commit_txs_hash.contains(&tx_hash_0));
        assert!(commit_txs_hash.contains(&tx_hash_1));
    }
}
