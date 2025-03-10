//! The ckb-ipc crate offers a set of tools and runtime support for IPC in CKB scripts. It includes necessary
//! dependencies and features to facilitate communication between different parts of a CKB script.
mod error;
mod packet;
mod pipe;
mod vlq;

pub use error::IpcError;
pub use packet::{Packet, RequestPacket, ResponsePacket};
pub use pipe::Pipe;
pub use vlq::{vlq_decode, vlq_encode};
