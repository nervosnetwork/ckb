use crate::{Uint32, Uint64};

/// Consecutive block number starting from 0.
///
/// This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](type.Uint64.html#examples).
pub type BlockNumber = Uint64;
/// Consecutive epoch number starting from 0.
///
/// This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](type.Uint64.html#examples).
pub type EpochNumber = Uint64;
/// The epoch indicator of a block. It shows which epoch the block is in, and the elapsed epoch fraction after adding this block.
///
/// This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](type.Uint64.html#examples).
///
/// The lower 56 bits of the epoch field are split into 3 parts (listed in the order from higher bits to lower bits):
///
/// * The highest 16 bits represent the epoch length
/// * The next 16 bits represent the current block index in the epoch, starting from 0.
/// * The lowest 24 bits represent the current epoch number.
///
/// Assume there's a block, which number is 11555 and in epoch 50. The epoch 50 starts from block
/// 11000 and have 1000 blocks. The epoch field for this particular block will then be 1,099,520,939,130,930,
/// which is calculated in the following way:
///
/// ```text
/// 50 | ((11555 - 11000) << 24) | (1000 << 40)
/// ```
pub type EpochNumberWithFraction = Uint64;
/// The capacity of a cell is the value of the cell in Shannons. It is also the upper limit of the cell occupied storage size where every 100,000,000 Shannons give 1-byte storage.
///
/// This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](type.Uint64.html#examples).
pub type Capacity = Uint64;
/// Count of cycles consumed by CKB VM to run scripts.
///
/// This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](type.Uint64.html#examples).
pub type Cycle = Uint64;
/// The fee rate is the ratio between fee and transaction weight in unit Shannon per 1,000 bytes.
///
/// Based on the context, the weight is either the transaction virtual bytes or serialization size in the block.
///
/// This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](type.Uint64.html#examples).
pub type FeeRate = Uint64;
/// The Unix timestamp in milliseconds (1 second is 1000 milliseconds).
///
/// For example, 1588233578000 is Thu, 30 Apr 2020 07:59:38 +0000
///
/// This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](type.Uint64.html#examples).
pub type Timestamp = Uint64;
/// The simple increasing integer version.
///
/// This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](type.Uint32.html#examples).
pub type Version = Uint32;
