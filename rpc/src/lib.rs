pub(crate) mod controller;
pub(crate) mod error;
pub(crate) mod module;
pub(crate) mod server;
pub(crate) mod service_builder;

#[cfg(test)]
mod test;

pub use crate::controller::RpcServerController;
pub use crate::server::RpcServer;
pub use crate::service_builder::ServiceBuilder;

pub type IoHandler = jsonrpc_pubsub::PubSubHandler<Option<crate::module::SubscriptionSession>>;
