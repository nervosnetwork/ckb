// file is loaded as a module multiple times，this behavior is intentional,
// in order to reuse the test cases
#![allow(clippy::duplicate_mod)]

mod archive;
pub(crate) mod utils;

mod ckb_2019;
mod ckb_2021;
#[path = "ckb_latest/mod.rs"]
mod ckb_2023;
