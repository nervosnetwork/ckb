use super::utils::assert_committed_at;
use crate::utils::assert_send_transaction_fail;
use crate::{Node, Spec};
use ckb_types::core::EpochNumberWithFraction;

const EPOCH_LENGTH: u64 = 10;
const MATURITY_BLOCKS: u64 = 5;

pub struct CellbaseMaturity;

impl Spec for CellbaseMaturity {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.mine_until_out_bootstrap_period();
        let source = node.get_tip_block();
        let transaction = node.new_transaction(source.transactions()[0].hash());
        let mature_epoch =
            source.epoch().to_rational() + node.consensus().cellbase_maturity().to_rational();

        // Maturity uses the current tip epoch, without proposal-window lookahead.
        for _ in 0..MATURITY_BLOCKS {
            assert!(node.get_tip_block().epoch().to_rational() < mature_epoch);
            assert_send_transaction_fail(node, &transaction, "CellbaseImmaturity");
            node.assert_tx_pool_size(0, 0);
            node.mine(1);
        }
        assert_eq!(node.get_tip_block().epoch().to_rational(), mature_epoch);
        assert_eq!(
            node.rpc_client()
                .send_transaction(transaction.data().into()),
            transaction.hash()
        );
        let committed_at =
            node.get_tip_block_number() + node.consensus().tx_proposal_window().closest() + 1;
        assert_committed_at(node, &transaction, committed_at);
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.genesis_epoch_length = Some(EPOCH_LENGTH);
        spec.params.epoch_duration_target = Some(EPOCH_LENGTH * 8);
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.cellbase_maturity =
            Some(EpochNumberWithFraction::new(0, MATURITY_BLOCKS, EPOCH_LENGTH).full_value());
    }
}
