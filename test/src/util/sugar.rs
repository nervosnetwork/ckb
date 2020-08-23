use crate::util::mine::mine_until_bool;
use crate::Node;

pub fn out_ibd_mode(nodes: &[Node]) {
    for node in nodes.iter() {
        mine_until_bool(node, || node.get_tip_block_number() > 0)
    }
}

pub fn out_bootstrap_period(nodes: &[Node]) {
    for node in nodes.iter() {
        let farthest = node.consensus().tx_proposal_window().farthest();
        let out_bootstrap_period = farthest + 1;
        let predicate = || node.get_tip_block_number() > out_bootstrap_period;
        mine_until_bool(node, predicate)
    }
}
