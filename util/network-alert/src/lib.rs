//! Network Alert
//! See https://en.bitcoin.it/wiki/Alert_system to learn the history of Bitcoin alert system.
//! We implement the alert system in CKB for urgent situation,
//! In CKB early stage we may meet the same crisis bugs that Bitcoin meet,
//! in urgent case, CKB core team can send an alert message across CKB P2P network,
//! the client will show the alert message, the other behaviors of CKB node will not change.
//!
//! Network Alert will be removed soon once the CKB network is considered mature.
//!
pub mod alert_relayer;
pub mod notifier;
#[cfg(test)]
mod tests;
pub mod verifier;

use std::time::Duration;

pub(crate) const BAD_MESSAGE_BAN_TIME: Duration = Duration::from_secs(5 * 60);
