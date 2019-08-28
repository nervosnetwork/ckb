use proc_macro_hack::proc_macro_hack;

pub use ckb_fixed_hash_core::{error, H160, H256, H512, H520};

#[proc_macro_hack]
pub use ckb_fixed_hash_hack::h160;
#[proc_macro_hack]
pub use ckb_fixed_hash_hack::h256;
#[proc_macro_hack]
pub use ckb_fixed_hash_hack::h512;
#[proc_macro_hack]
pub use ckb_fixed_hash_hack::h520;
