use crate::{Net, Spec};
use ckb_core::transaction::ProposalShortId;
use ckb_core::BlockNumber;
use log::info;

pub struct CellbaseImmatureTx;

impl Spec for CellbaseImmatureTx {
    fn run(&self, net: Net) {
        info!("Running CellbaseImmatureTx");
        let node = &net.nodes[0];

        info!("Generate 1 block");
        node.generate_block();

        info!("Use generated block's cellbase as tx input");
        let tip_block = node.get_tip_block();
        let tx = node.new_transaction(tip_block.transactions()[0].hash().clone());
        let transaction_hash = tx.hash();
        node.rpc_client()
            .enqueue_test_transaction((&tx).into())
            .call()
            .unwrap();

        info!("proposal tx");
        node.generate_block();
        let proposal_block = node.get_tip_block();

        info!("Generated tx should be included in next block's proposal txs");
        assert!(proposal_block
            .proposals()
            .iter()
            .any(|id| ProposalShortId::from_tx_hash(&transaction_hash).eq(id)));

        node.generate_block();
        let null_block = node.get_tip_block();
        assert_eq!(null_block.transactions().len(), 1);

        info!("Generated tx should not included in commit txs, because it is immature");
        node.generate_block();
        let commit_block = node.get_tip_block();
        assert!(!commit_block
            .transactions()
            .iter()
            .any(|tx| transaction_hash.eq(&tx.hash())));
    }

    fn num_nodes(&self) -> usize {
        1
    }

    fn cellbase_maturity(&self) -> Option<BlockNumber> {
        Some(10)
    }
}
