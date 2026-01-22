use serde::{Deserialize, Serialize};

/// Specifies the topic which to be added as active subscription.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// Subscribe new tip headers.
    NewTipHeader,
    /// Subscribe new tip blocks.
    NewTipBlock,
    /// Subscribe new transactions which are submitted to the pool.
    NewTransaction,
    /// Subscribe in-pool transactions which proposed on chain.
    ProposedTransaction,
    /// Subscribe transactions which are abandoned by tx-pool.
    RejectedTransaction,
    /// Subscribe to logs.
    Log,
}

/// Log entry received from the subscription.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct LogEntry {
    /// The log message.
    pub message: String,
    /// The log level.
    pub level: LogLevel,
}

/// Log level for log subscription.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Error level.
    Error,
    /// Warn level.
    Warn,
    /// Info level.
    Info,
    /// Debug level.
    Debug,
    /// Trace level.
    Trace,
}
