//! Provide BlockStatus
#![allow(missing_docs)]
use bitflags::bitflags;
bitflags! {
    pub struct BlockStatus: u32 {
        const UNKNOWN                 =     0;

        const HEADER_VALID            =     1;
        const BLOCK_RECEIVED          =     1 | Self::HEADER_VALID.bits << 1;
        const BLOCK_STORED            =     1 | Self::BLOCK_RECEIVED.bits << 1;
        const BLOCK_VALID             =     1 | Self::BLOCK_STORED.bits << 1;

        const BLOCK_INVALID           =     1 << 12;
    }
}
