//! One single-writer transaction-pool authority and its bounded asynchronous service.
//!
//! Expensive resolution, canonical VM verification, block-fact extraction,
//! template packing and external publication all run off-lock. A single short
//! in-memory write lock over `AuthorityState` validates off-lock premises,
//! evaluates membership/replacement/eviction policy directly against live
//! indexes, reserves bounded outbox notices, and commits all coupled mutations
//! atomically. Outbox activation and displaced payload destruction strictly
//! follow lock release.
//!
//! Start at [service] for request and worker coordination. Policy in [ingress],
//! [membership], [eviction], [waiting] and [chain] mutates [state::AuthorityState]
//! through [store::Store]; [notice] and [relay] deliver committed effects.

mod budget;
mod chain;
mod eviction;
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
mod ranks;
mod relay;
mod residency;
pub(crate) mod service;
mod state;
mod store;
mod template;
mod waiting;

pub(crate) use template::TemplateSource;

#[cfg(test)]
pub(crate) mod tests;
