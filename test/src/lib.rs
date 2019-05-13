mod net;
mod node;
mod rpc;
pub mod specs;
mod utils;

use ckb_core::BlockNumber;
use regex::Regex;

pub use net::Net;
pub use node::Node;
pub use specs::{Spec, TestProtocol};

// ckb doesn't support tx proposal window configuration, use a hardcoded value for integration test.
pub const DEFAULT_TX_PROPOSAL_WINDOW: (BlockNumber, BlockNumber) = (2, 10);

pub fn assert_regex_match(text: &str, regex: &str) {
    let re = Regex::new(regex).unwrap();
    assert!(re.is_match(text), "text = {}, regex = {}", text, regex);
}
