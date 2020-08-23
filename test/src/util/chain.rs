use crate::Node;
use ckb_types::core::{BlockNumber, BlockView, HeaderView};
use std::ops::Range;

pub fn forward_main_blocks(src_node: &Node, dst_node: &Node, range: Range<BlockNumber>) {
    submit_blocks(dst_node, &download_main_blocks(src_node, range))
}

pub fn submit_blocks(node: &Node, blocks: &[BlockView]) {
    for block in blocks.iter() {
        node.submit_block(block);
    }
}

pub fn download_main_blocks(node: &Node, range: Range<BlockNumber>) -> Vec<BlockView> {
    range
        .map(|number| node.get_block_by_number(number))
        .collect()
}

pub fn download_main_headers(node: &Node, range: Range<BlockNumber>) -> Vec<HeaderView> {
    range
        .map(|number| node.get_header_by_number(number))
        .collect()
}
