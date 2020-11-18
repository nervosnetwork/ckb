//! Provide several fixed-length binary data, aka fixed-sized hashes.
//!
//! # Notice
//!
//! **This is an internal crate used by crate [`ckb_fixed_hash`], do not use this crate directly.**
//!
//! All structs and the module [`error`](error/index.html) in this crate are re-exported in crate [`ckb_fixed_hash`].
//!
//! And you can found examples in crate [`ckb_fixed_hash`].
//!
//! [`ckb_fixed_hash`]: ../ckb_fixed_hash/index.html

pub mod error;

mod impls;
mod serde;
mod std_cmp;
mod std_convert;
mod std_default;
mod std_fmt;
mod std_hash;
mod std_str;

/// The 20-byte fixed-length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as a 0x-prefixed hex string.
#[derive(Clone)]
pub struct H160(pub [u8; 20]);

/// The 32-byte fixed-length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as a 0x-prefixed hex string.
#[derive(Clone)]
pub struct H256(pub [u8; 32]);

/// The 64-byte fixed-length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as a 0x-prefixed hex string.
#[derive(Clone)]
pub struct H512(pub [u8; 64]);

/// The 65-byte fixed-length binary data.
///
/// The name comes from the number of bits in the data.
///
/// In JSONRPC, it is encoded as a 0x-prefixed hex string.
#[derive(Clone)]
pub struct H520(pub [u8; 65]);
