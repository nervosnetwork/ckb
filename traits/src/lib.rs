//! Common traits for accessing blockchain data.
//!
//! This crate provides a collection of traits that define interfaces for accessing various types
//! of blockchain data, including cell data, block epochs, block extensions, and block headers.
//! These traits enable loose coupling between components that need data access.
mod cell_data_provider;
mod epoch_provider;
mod extension_provider;
mod header_provider;

pub use crate::cell_data_provider::CellDataProvider;
pub use crate::epoch_provider::{BlockEpoch, EpochProvider};
pub use crate::extension_provider::ExtensionProvider;
pub use crate::header_provider::*;
