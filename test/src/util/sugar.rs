use crate::util::chain::forward_main_blocks;
use crate::util::mining::{mine_until_out_bootstrap_period, mine_until_out_ibd_mode};
use crate::Node;

pub fn out_ibd_mode(nodes: &[Node]) {
    if let Some(node0) = nodes.first() {
        mine_until_out_ibd_mode(node0);
        if nodes.len() <= 1 {
            return;
        }

        let tip_number = node0.get_tip_block_number();
        let range = 1..tip_number + 1;
        for node in nodes.iter().skip(1) {
            forward_main_blocks(node0, node, range.clone());
        }
    }
}

pub fn out_bootstrap_period(nodes: &[Node]) {
    if let Some(node0) = nodes.first() {
        mine_until_out_bootstrap_period(node0);
        if nodes.len() <= 1 {
            return;
        }

        let tip_number = node0.get_tip_block_number();
        let range = 1..tip_number + 1;
        for node in nodes.iter().skip(1) {
            forward_main_blocks(node0, node, range.clone());
        }
    }
}
