//! Unified tx-pool ownership, membership, scheduling and effect authority.
//!
//! Production entry points are introduced only through the atomic P9.7g
//! cutover facade. Foundation-only constructors remain test-gated, so a
//! production caller cannot stamp synthetic chain or validation evidence into
//! an authority receipt while the runtime wiring is being completed.

mod ban;
mod chain;
mod chain_boundary;
mod dependency;
mod effect;
mod indexes;
mod ingress;
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
mod source;
mod state;
mod template;
mod template_driver;
mod topology;
mod validation;
mod work;
mod worker;

#[cfg(test)]
mod tests;
