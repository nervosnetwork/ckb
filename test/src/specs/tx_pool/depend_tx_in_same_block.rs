use crate::{Node, Spec};
use ckb_logger::info;
use ckb_types::{core::TransactionView, packed::ProposalShortId};

pub struct DepentTxInSameBlock;

impl Spec for DepentTxInSameBlock {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        info!("Generate 2 tx in same block");
        node0.mine_until_out_bootstrap_period();
        let tx_hash_0 = node0.generate_transaction();
        let tx = node0.new_transaction(tx_hash_0.clone());
        let tx_hash_1 = tx.hash();
        node0.rpc_client().send_transaction(tx.data().into());

        info!("Generated 2 tx should be included in the next block's proposals");
        let proposed = node0.mine_with_blocking(|template| template.proposals.len() != 2);
        let proposal_block = node0.get_tip_block();
        let proposal_ids: Vec<_> = proposal_block.union_proposal_ids_iter().collect();
        assert!(proposal_ids.contains(&ProposalShortId::from_tx_hash(&tx_hash_0)));
        assert!(proposal_ids.contains(&ProposalShortId::from_tx_hash(&tx_hash_1)));

        node0.mine_with_blocking(|template| template.number.value() != (proposed + 1));

        info!("Generated 2 tx should be included in the next + 2 block");
        node0.mine_with_blocking(|template| template.transactions.len() != 2);
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
