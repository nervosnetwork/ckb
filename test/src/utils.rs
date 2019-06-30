use crate::Net;
use bytes::Bytes;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{Header, HeaderBuilder, Seal};
use ckb_core::BlockNumber;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_protocol::{RelayMessage, SyncMessage};
use flatbuffers::FlatBufferBuilder;
use std::collections::HashSet;
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
pub fn build_compact_block_with_prefilled(block: &Block, prefilled: Vec<usize>) -> Bytes {
    let prefilled = prefilled.into_iter().collect();
    let fbb = &mut FlatBufferBuilder::new();
    let message = RelayMessage::build_compact_block(fbb, &block, &prefilled);
    fbb.finish(message, None);
    fbb.finished_data().into()
}

// Build compact block based on core block
pub fn build_compact_block(block: &Block) -> Bytes {
    let fbb = &mut FlatBufferBuilder::new();
    let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
    fbb.finish(message, None);
    fbb.finished_data().into()
}

pub fn build_block_transactions(block: &Block) -> Bytes {
    let fbb = &mut FlatBufferBuilder::new();
    let message =
        RelayMessage::build_block_transactions(fbb, block.header().hash(), block.transactions());
    fbb.finish(message, None);
    fbb.finished_data().into()
}

pub fn build_header(header: &Header) -> Bytes {
    let fbb = &mut FlatBufferBuilder::new();
    let message = SyncMessage::build_headers(fbb, &[header.clone()]);
    fbb.finish(message, None);
    fbb.finished_data().into()
}

pub fn build_block(block: &Block) -> Bytes {
    let fbb = &mut FlatBufferBuilder::new();
    let message = SyncMessage::build_block(fbb, block);
    fbb.finish(message, None);
    fbb.finished_data().into()
}

pub fn new_block_with_template(template: BlockTemplate) -> Block {
    let cellbase = template.cellbase.data;
    let header_builder = HeaderBuilder::default()
        .version(template.version.0)
        .number(template.number.0)
        .difficulty(template.difficulty.clone())
        .timestamp(template.current_time.0)
        .parent_hash(template.parent_hash)
        .seal(Seal::new(rand::random(), Bytes::new()))
        .dao(template.dao.into_bytes());

    BlockBuilder::default()
        .uncles(template.uncles)
        .transaction(cellbase)
        .transactions(template.transactions)
        .proposals(template.proposals)
        .header_builder(header_builder)
        .build()
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
    while let Ok(_) = net.receive_timeout(Duration::new(3, 0)) {}
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
