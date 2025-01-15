//! Onion service module

use std::net::SocketAddr;

/// Onion service module
pub mod onion_service;

#[cfg(test)]
mod tests;

/// Configuration for onion service
pub struct OnionServiceConfig {
    /// Tor server url: like: 127.0.0.1:9050
    pub onion_server: String,
    /// path to store onion private key, default is ./data/network/onion/onion_private_key
    pub onion_private_key_path: String,
    /// tor controllr url, example: 127.0.0.1:9050
    pub tor_controller: String,
    /// tor controller hashed password
    pub tor_password: Option<String>,
    /// onion service will bind to CKB's p2p listen address, default is "127.0.0.1:8115"
    /// if you want to use other address, you should set it to the address you want
    pub onion_service_target: SocketAddr,
}
