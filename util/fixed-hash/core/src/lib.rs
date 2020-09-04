pub mod error;

mod impls;
mod serde;
mod std_cmp;
mod std_convert;
mod std_default;
mod std_fmt;
mod std_hash;
mod std_str;

/// The 20-byte fixed length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as 0x-prefixed hex string.
#[derive(Clone)]
pub struct H160(pub [u8; 20]);
/// The 32-byte fixed length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as 0x-prefixed hex string.
#[derive(Clone)]
pub struct H256(pub [u8; 32]);
/// The 64-byte fixed length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as 0x-prefixed hex string.
#[derive(Clone)]
pub struct H512(pub [u8; 64]);
/// The 65-byte fixed length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as 0x-prefixed hex string.
#[derive(Clone)]
pub struct H520(pub [u8; 65]);
