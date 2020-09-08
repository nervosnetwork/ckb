use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/* Examples:
 * ```toml
 * [debug_console]
 * threads = 3
 * listen_address = "127.0.0.1:8200"
 * ```
 */
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub threads: usize,
    pub listen_address: Option<SocketAddr>,
}
