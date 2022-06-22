use ckb_types::core::BlockNumber;
use std::time::Duration;

pub const BAD_MESSAGE_BAN_TIME: Duration = Duration::from_secs(5 * 60);
pub const LAST_N_BLOCKS: BlockNumber = 100;
