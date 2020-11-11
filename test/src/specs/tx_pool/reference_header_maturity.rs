use crate::util::check::is_transaction_committed;
use crate::util::mining::{mine, mine_until_out_bootstrap_period};
use crate::utils::assert_send_transaction_fail;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::core::EpochNumberWithFraction;
use log::info;

const CELLBASE_MATURITY_VALUE: u64 = 3;

pub struct ReferenceHeaderMaturity;

impl Spec for ReferenceHeaderMaturity {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate DEFAULT_TX_PROPOSAL_WINDOW + 2 block");
        mine_until_out_bootstrap_period(node);
        info!("Use generated block's cellbase as tx input");
        let base_block = node.get_tip_block();

        let cellbase_maturity = EpochNumberWithFraction::from_full_value(CELLBASE_MATURITY_VALUE);

        {
            info!("Ensure cellbase is matured");
            let base_epoch = base_block.epoch();
            let threshold = cellbase_maturity.to_rational() + base_epoch.to_rational();
            loop {
                let tip_block = node.get_tip_block();
                let tip_epoch = tip_block.epoch();
                let current = tip_epoch.to_rational();
                if current < threshold {
                    if tip_epoch.number() < base_epoch.number() + cellbase_maturity.number() {
                        let remained_blocks_in_epoch = tip_epoch.length() - tip_epoch.index();
                        mine(node, remained_blocks_in_epoch);
                    } else {
                        mine(&node, 1);
                    }
                } else {
                    break;
                }
            }
        }

        info!("Reference tip block's header to test for maturity");
        let tip_block = node.get_tip_block();

        let tx = node.new_transaction(base_block.transactions()[0].hash());
        let tx = tx
            .data()
            .as_advanced_builder()
            .header_dep(tip_block.hash())
            .build();

        {
            let base_epoch = tip_block.epoch();
            let threshold = cellbase_maturity.to_rational() + base_epoch.to_rational();
            loop {
                let tip_block = node.get_tip_block();
                let tip_epoch = tip_block.epoch();
                let current = tip_epoch.to_rational();
                if current < threshold {
                    assert_send_transaction_fail(node, &tx, "ImmatureHeader");
                } else {
                    break;
                }
                if tip_epoch.number() < base_epoch.number() + cellbase_maturity.number() {
                    let remained_blocks_in_epoch = tip_epoch.length() - tip_epoch.index();
                    mine(node, remained_blocks_in_epoch);
                } else {
                    mine(&node, 1);
                }
            }
        }

        info!("Tx will be added to pending pool");
        let tx_hash = node.rpc_client().send_transaction(tx.data().into());
        assert_eq!(tx_hash, tx.hash());
        node.assert_tx_pool_size(1, 0);

        info!("Tx will be added to proposed pool");
        mine(&node, DEFAULT_TX_PROPOSAL_WINDOW.0);
        node.assert_tx_pool_size(0, 1);
        mine(&node, 1);
        node.assert_tx_pool_size(0, 0);

        info!("Tx will be eventually accepted on chain");
        mine(node, 5);
        assert!(is_transaction_committed(node, &tx));
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.cellbase_maturity = CELLBASE_MATURITY_VALUE;
        spec.params.epoch_duration_target = 30;
        spec.params.genesis_epoch_length = 5;
    }
}
