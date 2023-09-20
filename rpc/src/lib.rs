//! See [module](module/index.html) for the RPC methods documentation.

pub(crate) mod error;
pub(crate) mod server;
pub(crate) mod service_builder;
pub(crate) mod util;

pub mod module;

#[cfg(test)]
mod tests;

use jsonrpc_core::MetaIoHandler;
use jsonrpc_utils::pub_sub::Session;

pub use crate::error::RPCError;
pub use crate::server::RpcServer;
pub use crate::service_builder::ServiceBuilder;

#[doc(hidden)]
pub type IoHandler = MetaIoHandler<std::option::Option<Session>>;
