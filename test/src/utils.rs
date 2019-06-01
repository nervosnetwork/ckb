use crate::Net;
use bytes::Bytes;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{Header, HeaderBuilder, Seal};
use ckb_protocol::{RelayMessage, SyncMessage};
use flatbuffers::FlatBufferBuilder;
use jsonrpc_types::BlockTemplate;
use std::collections::HashSet;
use std::convert::TryInto;
use std::thread::sleep;
use std::time::{Duration, Instant};

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

pub fn build_header(header: &Header) -> Bytes {
    let headers = vec![header.clone()];
    let fbb = &mut FlatBufferBuilder::new();
    let message = SyncMessage::build_headers(fbb, &headers);
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
        .seal(Seal::new(rand::random(), Bytes::new()));
    BlockBuilder::default()
        .uncles(
            template
                .uncles
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()
                .expect("parse uncles failed"),
        )
        .transaction(cellbase.try_into().expect("parse cellbase failed"))
        .transactions(
            template
                .transactions
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()
                .expect("parse commit transactions failed"),
        )
        .proposals(
            template
                .proposals
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()
                .expect("parse proposal transactions failed"),
        )
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
    while let Ok(_) = net.receive_timeout(Duration::new(0, 100)) {}
}
