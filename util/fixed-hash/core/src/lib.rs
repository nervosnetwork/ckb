pub mod error;

mod impls;
mod serde;
mod std_cmp;
mod std_convert;
mod std_default;
mod std_fmt;
mod std_hash;
mod std_str;

#[derive(Clone)]
pub struct H160(pub [u8; 20]);
#[derive(Clone)]
pub struct H256(pub [u8; 32]);
#[derive(Clone)]
pub struct H512(pub [u8; 64]);
#[derive(Clone)]
pub struct H520(pub [u8; 65]);
