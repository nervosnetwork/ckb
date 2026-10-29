//! Shared behavioral tests for the single Store and its public execution paths.
//! Tests needing a production module's private items are registered by that module.
mod budget;
pub(in crate::authority) mod common;
mod contracts;
pub(in crate::authority) mod entry_metrics;
mod ingress_contracts;
mod membership;
mod membership_aggregates;
mod relay;
mod residency;
mod waiting;
