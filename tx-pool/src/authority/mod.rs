//! Unified tx-pool ownership, membership, scheduling and effect authority.
//!
//! Production entry points expose only validated commands and move-only
//! capabilities. Foundation-only constructors remain test-gated, so a
//! production caller cannot stamp synthetic chain or validation evidence into
//! an authority receipt.

mod ban;
mod chain;
mod chain_boundary;
mod compute_coordinator;
mod dependency;
mod effect;
mod exchange;
mod indexes;
mod ingress;
pub(crate) use ingress::{BoundedTransaction, BoundedTransactionError};
#[cfg(any(test, feature = "internal"))]
mod internal;
mod packing;
mod plan;
mod publisher;
pub(crate) mod query;
mod read;
mod rejection;
mod relay;
mod residency;
mod resolver;
mod resources;
pub(crate) mod runtime;
mod scheduler;
pub(crate) mod service;
mod shard;
mod source;
mod state;
mod template;
mod template_driver;
mod topology;
mod validation;
mod work;
mod worker;

#[cfg(test)]
#[path = "tests/support/shard_support.rs"]
mod shard_support;
#[cfg(test)]
mod tests;
