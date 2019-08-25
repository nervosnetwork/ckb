use crate::{Net, Spec};
use ckb_types::{core::TransactionView, packed::ProposalShortId, prelude::*};
use log::info;

pub struct DepentTxInSameBlock;

impl Spec for DepentTxInSameBlock {
    crate::name!("depent_tx_in_same_block");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];

        info!("Generate 2 tx in same block");
        node0.generate_block();
        let tx_hash_0 = node0.generate_transaction();
        let tx = node0.new_transaction(tx_hash_0.clone());
        let tx_hash_1 = tx.hash().unpack();
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

        assert!(commit_txs_hash.contains(&tx_hash_0.pack()));
        assert!(commit_txs_hash.contains(&tx_hash_1.pack()));
    }
}
