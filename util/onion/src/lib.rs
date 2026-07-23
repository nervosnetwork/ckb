//! Onion service module

use std::net::SocketAddr;

mod onion;
/// Onion service module
pub mod onion_service;
mod tor_connection;
/// Tor controller module
pub mod tor_controller;

pub use onion::{OnionAddressV3, TorPublicKeyV3, TorSecretKeyV3};
pub use tor_connection::ConnError;
pub use tor_controller::TorController;

/// Configuration for onion service
pub struct OnionServiceConfig {
    /// path to store onion private key, default is ./data/network/onion_private_key
    pub onion_private_key_path: String,
    /// tor controller url, example: 127.0.0.1:9051
    pub tor_controller: String,
    /// tor controller hashed password
    pub tor_password: Option<String>,
    /// onion service will bind to CKB's p2p listen address, default is "127.0.0.1:8115"
    /// if you want to use other address, you should set it to the address you want
    pub p2p_listen_address: SocketAddr,
    /// The external port that the onion service will expose, default is 8115
    /// This is the port that will be advertised in the onion address,
    /// while traffic will be forwarded to `p2p_listen_address`.
    pub onion_external_port: u16,
}
