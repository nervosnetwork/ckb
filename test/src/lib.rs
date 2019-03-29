mod net;
mod node;
mod rpc;
pub mod specs;

use std::thread;
use std::time;

pub use net::Net;
pub use node::{Node, TestNode};
pub use specs::Spec;

pub fn sleep(secs: u64) {
    thread::sleep(time::Duration::from_secs(secs));
}
