//! Shared behavioral tests for the single Store and its public execution paths.
//! Tests needing a production module's private items are registered by that module.
mod budget;
pub(in crate::authority) mod common;
mod contracts;
mod ingress_contracts;
mod membership;
mod relay;
mod residency;
mod waiting;
