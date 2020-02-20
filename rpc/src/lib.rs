pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod module;
pub(crate) mod server;
pub(crate) mod service_builder;

#[cfg(test)]
mod test;

pub use crate::config::{Config, Module};
pub use crate::server::RpcServer;
pub use crate::service_builder::ServiceBuilder;

pub type IoHandler = jsonrpc_pubsub::PubSubHandler<
    Option<crate::module::SubscriptionSession>,
    server::ModuleEnableCheck,
>;
