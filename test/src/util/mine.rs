use crate::Node;
use ckb_types::core::{BlockBuilder, BlockView};
use ckb_types::packed;

pub fn mine_until_out_ibd_mode(node: &Node) {
    mine_until_bool(node, || node.get_tip_block_number() > 0)
}

/// The `[1, PROPOSAL_WINDOW.farthest()]` of chain is called as bootstrap period. Cellbases w
/// this period are zero capacity.
///
/// This function will generate blank blocks until node.tip_block_number > PROPOSAL_WINDOW.fa
///
/// Typically involve this function at the beginning of test.
pub fn mine_until_out_bootstrap_period(node: &Node) {
    let farthest = node.consensus().tx_proposal_window().farthest();
    let out_bootstrap_period = farthest + 1;
    let predicate = || node.get_tip_block_number() > out_bootstrap_period;
    mine_until_bool(node, predicate)
}

pub fn mine(node: &Node, count: u64) {
    let with = |builder: BlockBuilder| builder.build();
    mine_with(node, count, with)
}

pub fn mine_with<W>(node: &Node, count: u64, with: W)
where
    W: Fn(BlockBuilder) -> BlockView,
{
    for _ in 0..count {
        let template = node.rpc_client().get_block_template(None, None, None);
        let builder = packed::Block::from(template).as_advanced_builder();
        let block = with(builder);
        node.submit_block(&block);
    }
}

pub fn mine_until_bool<P>(node: &Node, predicate: P)
where
    P: Fn() -> bool,
{
    let until = || if predicate() { Some(()) } else { None };
    mine_until(node, until)
}

pub fn mine_until<T, U>(node: &Node, until: U) -> T
where
    U: Fn() -> Option<T>,
{
    let with = |builder: BlockBuilder| builder.build();
    mine_until_with(node, until, with)
}

pub fn mine_until_with<W, T, U>(node: &Node, until: U, with: W) -> T
where
    U: Fn() -> Option<T>,
    W: Fn(BlockBuilder) -> BlockView,
{
    loop {
        if let Some(t) = until() {
            return t;
        }

        let template = node.rpc_client().get_block_template(None, None, None);
        let builder = packed::Block::from(template).as_advanced_builder();
        let block = with(builder);
        node.submit_block(&block);
    }
}
