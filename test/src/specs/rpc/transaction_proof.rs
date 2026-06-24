use crate::{DEFAULT_TX_PROPOSAL_WINDOW, Node, Spec};
use ckb_jsonrpc_types::Uint32;

pub struct RpcTransactionProof;

impl Spec for RpcTransactionProof {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        node0.mine(DEFAULT_TX_PROPOSAL_WINDOW.1 + 2);

        let tx_hash = node0.generate_transaction();
        node0.mine_until_transaction_confirm(&tx_hash);
        let tx_hashes = vec![tx_hash.into()];

        // ---- verify_transaction_proof ----

        // Valid proof must be accepted.
        let proof = node0
            .rpc_client()
            .inner()
            .get_transaction_proof(tx_hashes.clone(), None)
            .expect("get_transaction_proof should be ok");
        let verified_tx_hashes = node0
            .rpc_client()
            .inner()
            .verify_transaction_proof(proof.clone())
            .expect("verify_transaction_proof should be ok");
        assert_eq!(tx_hashes, verified_tx_hashes);

        // Reject empty indices.
        {
            let mut invalid = proof.clone();
            invalid.proof.indices = vec![];
            assert!(
                node0
                    .rpc_client()
                    .inner()
                    .verify_transaction_proof(invalid)
                    .is_err(),
                "should reject empty indices"
            );
        }

        // Reject duplicate indices.
        {
            let mut invalid = proof.clone();
            let idx = invalid.proof.indices[0];
            invalid.proof.indices = vec![idx, idx, idx];
            assert!(
                node0
                    .rpc_client()
                    .inner()
                    .verify_transaction_proof(invalid)
                    .is_err(),
                "should reject duplicate indices"
            );
        }

        // Reject indices exceeding the block transaction count.
        {
            let mut invalid = proof.clone();
            invalid.proof.indices = (0..10000u32).map(Uint32::from).collect();
            assert!(
                node0
                    .rpc_client()
                    .inner()
                    .verify_transaction_proof(invalid)
                    .is_err(),
                "should reject oversized indices"
            );
        }

        // ---- verify_transaction_and_witness_proof ----

        // Valid TransactionAndWitnessProof must be accepted.
        let tx_and_witness = node0
            .rpc_client()
            .inner()
            .get_transaction_and_witness_proof(tx_hashes.clone(), None)
            .expect("get_transaction_and_witness_proof should be ok");
        let verified = node0
            .rpc_client()
            .inner()
            .verify_transaction_and_witness_proof(tx_and_witness.clone())
            .expect("verify_transaction_and_witness_proof should be ok");
        assert_eq!(tx_hashes, verified);

        // Reject duplicate transaction proof indices.
        {
            let mut invalid = tx_and_witness.clone();
            let idx = invalid.transactions_proof.indices[0];
            invalid.transactions_proof.indices = vec![idx, idx, idx];
            assert!(
                node0
                    .rpc_client()
                    .inner()
                    .verify_transaction_and_witness_proof(invalid)
                    .is_err(),
                "should reject duplicate transaction proof indices"
            );
        }

        // Reject duplicate witness proof indices.
        {
            let mut invalid = tx_and_witness.clone();
            let idx = invalid.witnesses_proof.indices[0];
            invalid.witnesses_proof.indices = vec![idx, idx, idx];
            assert!(
                node0
                    .rpc_client()
                    .inner()
                    .verify_transaction_and_witness_proof(invalid)
                    .is_err(),
                "should reject duplicate witness proof indices"
            );
        }

        // Reject mismatched transaction and witness index sets.
        // After uniqueness validation both sets must contain the same indices.
        {
            let mut invalid = tx_and_witness.clone();
            invalid.witnesses_proof.indices = vec![Uint32::from(
                invalid.transactions_proof.indices[0]
                    .value()
                    .wrapping_add(1),
            )];
            assert!(
                node0
                    .rpc_client()
                    .inner()
                    .verify_transaction_and_witness_proof(invalid)
                    .is_err(),
                "should reject mismatched transaction/witness index sets"
            );
        }

        // Reject oversized witness proof indices.
        {
            let mut invalid = tx_and_witness.clone();
            invalid.witnesses_proof.indices = (0..10000u32).map(Uint32::from).collect();
            assert!(
                node0
                    .rpc_client()
                    .inner()
                    .verify_transaction_and_witness_proof(invalid)
                    .is_err(),
                "should reject oversized witness proof indices"
            );
        }
    }
}
