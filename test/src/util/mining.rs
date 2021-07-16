use crate::util::chain::forward_main_blocks;
use crate::Node;
use ckb_types::{
    core::{BlockBuilder, BlockView, EpochNumberWithFraction, HeaderView},
    packed,
};

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
    // TODO predicate by output.is_some() is more realistic. But keeps original behaviours,
    // update it later.
    // let predicate = || {
    //     node.get_tip_block()
    //         .transaction(0)
    //         .map(|tx| tx.output(0).is_some())
    //         .unwrap_or(false)
    // };

    let farthest = node.consensus().tx_proposal_window().farthest();
    let out_bootstrap_period = farthest + 2;
    let predicate = || node.get_tip_block_number() >= out_bootstrap_period;
    mine_until_bool(node, predicate)
}

pub fn mine_until_epoch(node: &Node, number: u64, index: u64, length: u64) {
    let target_epoch = EpochNumberWithFraction::new(number, index, length);
    mine_until_bool(node, || {
        let tip_header: HeaderView = node.rpc_client().get_tip_header().into();
        let tip_epoch = tip_header.epoch();
        if tip_epoch > target_epoch {
            panic!(
                "expect mine until epoch {} but already be epoch {}",
                target_epoch, tip_epoch
            );
        }
        tip_epoch == target_epoch
    });
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
