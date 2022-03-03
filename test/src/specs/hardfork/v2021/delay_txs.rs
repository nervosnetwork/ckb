use crate::{Node, Spec};

const GENESIS_EPOCH_LENGTH: u64 = 20;
const CKB2021_START_EPOCH: u64 = 2;

pub struct DelayTxs;

impl Spec for DelayTxs {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let epoch_length = GENESIS_EPOCH_LENGTH;

        let node = &nodes[0];

        let proposal_window = node.consensus().tx_proposal_window();
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;

        node.mine_until_epoch(
            ckb2019_last_epoch,
            epoch_length - proposal_window.farthest(),
            epoch_length,
        );

        let delay_windows = proposal_window.farthest() * 2 + 1;

        for _ in 0..delay_windows {
            node.wait_for_tx_pool();

            let tx = node.new_transaction_spend_tip_cellbase();
            node.submit_transaction(&tx);

            let ret = node.rpc_client().get_transaction(tx.hash());
            assert!(ret.is_none(), "tx should be delayed");

            node.mine(1);
        }
        // tx should be processed after delay_windows
        // but in order to avoid asynchronous non-determinism
        // we check in next block.
        node.mine(1);
        node.wait_for_tx_pool();
        node.assert_tx_pool_size(delay_windows, 0);
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if spec.params.hardfork.is_none() {
            spec.params.hardfork = Some(Default::default());
        }
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_0032 = Some(CKB2021_START_EPOCH);
        }
    }
}
