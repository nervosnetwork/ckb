use bytes::Bytes;
use ckb_core::block::Block;
use ckb_protocol::RelayMessage;
use flatbuffers::FlatBufferBuilder;
use std::collections::HashSet;
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
