//! block_status for block_status_map
//! BlockStatus for Sync protocol
//! https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0004-ckb-block-sync/0004-ckb-block-sync.md#abstract
use bitflags::bitflags;

bitflags! {
#[doc(hidden)]
    pub struct BlockStatus: u32 {
        const UNKNOWN                 =     0;

        const HEADER_VALID            =     1;
        const BLOCK_RECEIVED          =     Self::HEADER_VALID.bits | 1 << 1;
        const BLOCK_STORED            =     Self::HEADER_VALID.bits | Self::BLOCK_RECEIVED.bits | 1 << 3;
        const BLOCK_VALID             =     Self::HEADER_VALID.bits | Self::BLOCK_RECEIVED.bits | Self::BLOCK_STORED.bits | 1 << 4;

        const BLOCK_INVALID           =     1 << 12;
    }
}
