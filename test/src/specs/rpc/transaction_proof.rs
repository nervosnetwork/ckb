use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::prelude::*;

pub struct RpcTransactionProof;

impl Spec for RpcTransactionProof {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        node0.mine(DEFAULT_TX_PROPOSAL_WINDOW.1 + 2);

        let tx_hash = node0.generate_transaction().unpack();
        let tx_hashes = vec![tx_hash];
        node0.mine_until_transactions_confirm();
        let proof = node0
            .rpc_client()
            .inner()
            .get_transaction_proof(tx_hashes.clone(), None)
            .expect("get_transaction_proof should be ok");
        let verified_tx_hashes = node0
            .rpc_client()
            .inner()
            .verify_transaction_proof(proof)
            .expect("verify_transaction_proof should be ok");
        assert_eq!(tx_hashes, verified_tx_hashes);
    }
}
