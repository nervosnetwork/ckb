extern crate bigint;
extern crate flatbuffers;
extern crate jsonrpc_core;
#[macro_use]
extern crate jsonrpc_macros;
extern crate jsonrpc_http_server;
extern crate jsonrpc_server_utils;
#[macro_use]
extern crate log;
extern crate ckb_core;
#[cfg(test)]
extern crate ckb_db;
extern crate ckb_network;
extern crate ckb_notify;
extern crate ckb_pool;
extern crate ckb_protocol;
extern crate ckb_shared;
extern crate ckb_sync;
extern crate ckb_time;
#[cfg(test)]
extern crate ckb_verification;
#[macro_use]
extern crate serde_derive;
#[cfg(feature = "integration_test")]
extern crate ckb_pow;
#[macro_use]
extern crate crossbeam_channel as channel;
extern crate fnv;

mod server;
mod service;
mod types;

pub use service::{RpcController, RpcReceivers, RpcService};
pub use types::{BlockTemplate, Config};

#[cfg(feature = "integration_test")]
mod integration_test;

#[cfg(feature = "integration_test")]
pub use integration_test::RpcServer;
#[cfg(not(feature = "integration_test"))]
pub use server::RpcServer;
