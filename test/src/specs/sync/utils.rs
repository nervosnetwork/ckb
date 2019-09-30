use crate::utils::wait_until;
use crate::Net;
use ckb_types::{
    packed::{GetBlocks, SyncMessage},
    prelude::*,
};
use std::time::Duration;

pub fn wait_get_blocks(secs: u64, net: &Net) -> bool {
    wait_until(secs, || {
        if let Ok((_, _, data)) = net.receive_timeout(Duration::from_secs(1)) {
            if let Ok(message) = SyncMessage::from_slice(&data) {
                return message.to_enum().item_name() == GetBlocks::NAME;
            }
        }
        false
    })
}
