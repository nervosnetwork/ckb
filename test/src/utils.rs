use crate::{Net, Node};
use ckb_jsonrpc_types::{BlockTemplate, TransactionWithStatus, TxStatus};
use ckb_types::{
    bytes::Bytes,
    core::{BlockNumber, BlockView, HeaderView, TransactionView},
    packed::{
        Block, BlockTransactions, CompactBlock, GetBlocks, RelayMessage, SendBlock, SendHeaders,
        SyncMessage,
    },
    prelude::*,
    H256,
};
use std::borrow::Borrow;
use std::convert::Into;
use std::thread::sleep;
use std::time::{Duration, Instant};

pub const MEDIAN_TIME_BLOCK_COUNT: u64 = 11;
pub const FLAG_SINCE_RELATIVE: u64 =
    0b1000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
pub const FLAG_SINCE_BLOCK_NUMBER: u64 =
    0b000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
// pub const FLAG_SINCE_EPOCH_NUMBER: u64 =
//    0b010_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
pub const FLAG_SINCE_TIMESTAMP: u64 =
    0b100_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;

// Build compact block based on core block, and specific prefilled indices
pub fn build_compact_block_with_prefilled(block: &BlockView, prefilled: Vec<usize>) -> Bytes {
    let prefilled = prefilled.into_iter().collect();
    let compact_block = CompactBlock::build_from_block(block, &prefilled);
    RelayMessage::new_builder()
        .set(compact_block)
        .build()
        .as_bytes()
}

// Build compact block based on core block
pub fn build_compact_block(block: &BlockView) -> Bytes {
    build_compact_block_with_prefilled(block, Vec::new())
}

pub fn build_block_transactions(block: &BlockView) -> Bytes {
    // compact block has always prefilled cellbase
    let block_txs = BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(
            block
                .transactions()
                .into_iter()
                .map(|view| view.data())
                .skip(1)
                .pack(),
        )
        .build();
    RelayMessage::new_builder()
        .set(block_txs)
        .build()
        .as_bytes()
}

pub fn build_header(header: &HeaderView) -> Bytes {
    build_headers(&[header.clone()])
}

pub fn build_headers(headers: &[HeaderView]) -> Bytes {
    let send_headers = SendHeaders::new_builder()
        .headers(
            headers
                .iter()
                .map(|view| view.data())
                .collect::<Vec<_>>()
                .pack(),
        )
        .build();
    SyncMessage::new_builder()
        .set(send_headers)
        .build()
        .as_bytes()
}

pub fn build_block(block: &BlockView) -> Bytes {
    SyncMessage::new_builder()
        .set(SendBlock::new_builder().block(block.data()).build())
        .build()
        .as_bytes()
}

pub fn build_get_blocks(hashes: &[H256]) -> Bytes {
    let get_blocks = GetBlocks::new_builder()
        .block_hashes(hashes.iter().map(|hash| hash.pack()).pack())
        .build();
    SyncMessage::new_builder()
        .set(get_blocks)
        .build()
        .as_bytes()
}

pub fn new_block_with_template(template: BlockTemplate) -> Block {
    Block::from(template)
        .as_advanced_builder()
        .nonce(rand::random::<u64>().pack())
        .build()
        .data()
}

pub fn wait_until<F>(secs: u64, mut f: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    let timeout = Duration::new(secs, 0);
    while Instant::now().duration_since(start) <= timeout {
        if f() {
            return true;
        }
        sleep(Duration::new(1, 0));
    }
    false
}

// Clear net message channel
pub fn clear_messages(net: &Net) {
    while let Ok(_) = net.recv_timeout(Duration::new(3, 0)) {}
}

pub fn since_from_relative_block_number(block_number: BlockNumber) -> u64 {
    FLAG_SINCE_RELATIVE | FLAG_SINCE_BLOCK_NUMBER | block_number
}

pub fn since_from_absolute_block_number(block_number: BlockNumber) -> u64 {
    FLAG_SINCE_BLOCK_NUMBER | block_number
}

// pub fn since_from_relative_epoch_number(epoch_number: EpochNumber) -> u64 {
//     FLAG_SINCE_RELATIVE | FLAG_SINCE_EPOCH_NUMBER | epoch_number
// }
//
// pub fn since_from_absolute_epoch_number(epoch_number: EpochNumber) -> u64 {
//     FLAG_SINCE_EPOCH_NUMBER | epoch_number
// }

pub fn since_from_relative_timestamp(timestamp: u64) -> u64 {
    FLAG_SINCE_RELATIVE | FLAG_SINCE_TIMESTAMP | timestamp
}

pub fn since_from_absolute_timestamp(timestamp: u64) -> u64 {
    FLAG_SINCE_TIMESTAMP | timestamp
}

pub fn assert_send_transaction_fail(node: &Node, transaction: &TransactionView, message: &str) {
    let result = node
        .rpc_client()
        .inner()
        .lock()
        .send_transaction(transaction.data().into())
        .call();
    let error = result.expect_err(&format!("transaction is invalid since {}", message));
    let error_string = error.to_string();
    assert!(
        error_string.contains(message),
        "expect error \"{}\" but got \"{}\"",
        message,
        error_string,
    );
}

pub fn is_committed(tx_status: &TransactionWithStatus) -> bool {
    let committed_status = TxStatus::committed(H256::zero());
    tx_status.tx_status.status == committed_status.status
}

/// Workaround for banned address checking (because we are using loop-back addresses)
///   1. checking banned addresses is empty
///   2. connecting outbound peer and checking banned addresses is not empty
///   3. clear banned addresses
pub fn connect_and_wait_ban(inbound: &Node, outbound: &Node) {
    assert!(
        inbound.rpc_client().get_banned_addresses().is_empty(),
        "banned addresses should be empty"
    );

    let outbound_info = outbound.rpc_client().local_node_info();
    let outbound_id = outbound_info.node_id;
    inbound.rpc_client().add_node(
        outbound_id.clone(),
        format!("/ip4/127.0.0.1/tcp/{}", outbound.p2p_port()),
    );

    let banned = wait_until(10, || {
        !inbound.rpc_client().get_banned_addresses().is_empty()
    });
    assert!(
        !banned,
        "connect_and_wait_ban timeout, inbound_id: {}, outbound_id: {}",
        inbound.node_id().as_ref().unwrap(),
        outbound.node_id().as_ref().unwrap(),
    );

    let banned_addresses = inbound.rpc_client().get_banned_addresses();
    banned_addresses.into_iter().for_each(|ban_address| {
        inbound
            .rpc_client()
            .set_ban(ban_address.address, "delete".to_owned(), None, None, None)
    });
}

pub fn waiting_for_sync<N>(nodes: &[N], expected: BlockNumber)
where
    N: Borrow<Node>,
{
    // 60 seconds is a reasonable timeout to sync, even for poor CI server
    let synced = wait_until(60, || {
        nodes
            .iter()
            .map(|node| node.borrow().get_tip_block_number())
            .all(|tip_number| tip_number == expected)
    });
    assert!(
        synced,
        "waiting_for_sync timeout, expected: {}, actual: {:?}",
        expected,
        nodes
            .iter()
            .map(|node| node.borrow().get_tip_block_number())
            .collect::<Vec<_>>(),
    );
}

pub fn waiting_for_sync2(node_a: &Node, node_b: &Node, expected: BlockNumber) {
    waiting_for_sync(&[node_a, node_b], expected)
}

pub fn assert_tx_pool_size(node: &Node, pending_size: u64, proposed_size: u64) {
    let tx_pool_info = node.rpc_client().tx_pool_info();
    assert_eq!(tx_pool_info.pending.0, pending_size);
    assert_eq!(tx_pool_info.proposed.0, proposed_size);
}

pub fn assert_tx_pool_statics(node: &Node, total_tx_size: u64, total_tx_cycles: u64) {
    let tx_pool_info = node.rpc_client().tx_pool_info();
    assert_eq!(tx_pool_info.total_tx_size.0, total_tx_size);
    assert_eq!(tx_pool_info.total_tx_cycles.0, total_tx_cycles);
}

/// All nodes disconnect each other
pub fn disconnect_all(nodes: &[Node]) {
    for i in 0..nodes.len() {
        for j in i + 1..nodes.len() {
            nodes[i].disconnect(&nodes[j]);
            nodes[j].disconnect(&nodes[i]);
        }
    }
}

/// All nodes connect each other
pub fn connect_all(nodes: &[Node]) {
    nodes
        .windows(2)
        .for_each(|nodes| nodes[0].connect(&nodes[1]));
}

/// All nodes mine a same block to exit the IBD mode
pub fn exit_ibd_mode(nodes: &[Node]) -> BlockView {
    let block = nodes[0].new_block(None, None, None);
    nodes.iter().for_each(|node| {
        node.submit_block(&block.data());
    });
    block
}
