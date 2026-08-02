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
mod plan;
mod publisher;
mod read;
mod rejection;
mod resolver;
mod resources;
pub(crate) mod runtime;
mod scheduler;
mod source;
mod state;
mod template;
mod validation;
mod work;
mod worker;

#[cfg(test)]
pub(crate) use rejection::bounded_recent_reject;
pub(crate) use rejection::{
    MAX_COMMIT_BAN_REASON_BYTES, MAX_RECENT_REJECT_BYTES, bounded_commit_ban_reason,
    serialized_recent_reject,
};

#[cfg(test)]
mod tests;
