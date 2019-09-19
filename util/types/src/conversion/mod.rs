//! Conversions between packed bytes wrappers and rust structs.
//!
//! ### Warning
//!
//! No more logic is allowed, except serialize and deserialize.

#[macro_use]
mod utilities;

mod blockchain;
mod network;
mod primitive;
mod storage;
