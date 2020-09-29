use serde::{Deserialize, Serialize};

/// Runtime logger config for extra loggers.
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct ExtraLoggerConfig {
    /// Sets log levels for different modules.
    ///
    /// ## Examples
    ///
    /// Set the log level to info for all modules
    ///
    /// ```text
    /// info
    /// ```
    ///
    /// Set the log level to debug for listed modules and info for other modules.
    ///
    /// ```text
    /// info,ckb-rpc=debug,ckb-sync=debug,ckb-relay=debug,ckb-tx-pool=debug,ckb-network=debug
    /// ```
    pub filter: String,
}

/// Runtime logger config.
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct MainLoggerConfig {
    /// Sets log levels for different modules.
    ///
    /// **Optional**, null means keeping the current option unchanged.
    ///
    /// ## Examples
    ///
    /// Set the log level to info for all modules
    ///
    /// ```text
    /// info
    /// ```
    ///
    /// Set the log level to debug for listed modules and info for other modules.
    ///
    /// ```text
    /// info,ckb-rpc=debug,ckb-sync=debug,ckb-relay=debug,ckb-tx-pool=debug,ckb-network=debug
    /// ```
    pub filter: Option<String>,
    /// Whether printing the logs to the process stdout.
    ///
    /// **Optional**, null means keeping the current option unchanged.
    pub to_stdout: Option<bool>,
    /// Whether appending the logs to the log file.
    ///
    /// **Optional**, null means keeping the current option unchanged.
    pub to_file: Option<bool>,
    /// Whether using color when printing the logs to the process stdout.
    ///
    /// **Optional**, null means keeping the current option unchanged.
    pub color: Option<bool>,
}
