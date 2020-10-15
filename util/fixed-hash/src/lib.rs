//! Provide several simple fixed-sized hash data type and their static constructors.
//!
//! # Example
//!
//! ```rust
//! use ckb_fixed_hash::{H256, h256};
//!
//! const N1: H256 = h256!("0xffffffff_ffffffff_ffffffff_fffffffe_baaedce6_af48a03b_bfd25e8c_d0364141");
//! const N2: H256 = H256([
//!     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
//!     0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
//!     0x41, 0x41
//! ]);
//! assert_eq!(N1, N2);
//!
//! const ONE1: H256 = h256!("0x1");
//! const ONE2: H256 = H256([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
//! assert_eq!(ONE1, ONE2);
//! ```

pub use ckb_fixed_hash_core::{error, H160, H256, H512, H520};
pub use ckb_fixed_hash_macros::{h160, h256, h512, h520};
