pub mod assertion;
pub mod generic;
pub mod global;
mod net;
mod node;
mod rpc;
pub mod specs;
mod txo;
pub mod util;
pub mod utils;
pub mod worker;

use ckb_types::core::BlockNumber;

pub use net::Net;
pub use node::Node;
pub use specs::{Setup, Spec};
pub use txo::{TXOSet, TXO};

// ckb doesn't support tx proposal window configuration, use a hardcoded value for integration test.
pub const DEFAULT_TX_PROPOSAL_WINDOW: (BlockNumber, BlockNumber) = (2, 10);
pub const FINALIZATION_DELAY_LENGTH: BlockNumber = DEFAULT_TX_PROPOSAL_WINDOW.1 + 1;
pub const SYSTEM_CELL_ALWAYS_SUCCESS_INDEX: u32 = 5;
