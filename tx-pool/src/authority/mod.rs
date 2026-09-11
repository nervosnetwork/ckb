//! One sharded transaction store and its bounded asynchronous service.
//!
//! Immutable owners are the facts. A single synchronous commit checks the
//! original reads and changes owners, derived indexes, quota and committed
//! notices together. Workers keep their work through admission; publication
//! and every external call follow guard release.

mod budget;
mod chain;
mod ingress;
mod jobs;
mod membership;
mod model;
mod notice;
mod packing;
#[cfg(feature = "packing-bench")]
#[path = "../../benches/packing/current_adapter.rs"]
pub mod packing_bench;
pub(crate) mod query;
mod queue;
mod relay;
mod residency;
pub(crate) mod service;
mod store;
mod template;
mod waiting;

pub(crate) use template::TemplateSource;

#[cfg(test)]
mod tests;
