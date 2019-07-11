#[macro_use]
mod macros;
mod chain;
mod mock_store;

pub use chain::{always_success_cell, always_success_cellbase, always_success_consensus};
pub use mock_store::MockStore;
