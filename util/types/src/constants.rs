//! All Constants.

use crate::{core::Version, h256, H256};

pub const TX_VERSION: Version = 0;
pub const HEADER_VERSION: Version = 0;
pub const BLOCK_VERSION: Version = 0;
// "TYPE_ID" in hex
pub const TYPE_ID_CODE_HASH: H256 = h256!("0x545950455f4944");
