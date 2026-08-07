//! Independent executable reference model for the tx-pool proof contract.
//!
//! This module deliberately imports no production authority, planner, worker,
//! or protocol type. Shared names describe public semantics only; generated
//! contract checks bind those names to production separately.

mod adversarial;
mod adversarial_properties;
mod boundaries;
mod composition;
mod composition_properties;
mod handoff;
mod kernel;
mod permit;
mod properties;
mod protocol;
mod resource;
mod state;
