//! Provide several proc-macros to construct const fixed-sized hashes.
//!
//! If we use an array to construct const fixed-sized hashes, it's difficult to read.
//!
//! If we use [`FromStr::from_str`] to construct fixed-sized hashes, the result is not a constant.
//! So, it will reduce runtime performance. And it could cause a runtime error if the input is malformed.
//!
//! With proc-macros, we can construct human-readable const fixed-sized hashes.
//! And it will be checked in compile time, it could never cause any runtime error.
//!
//! # Notice
//!
//! **This is an internal crate used by crate [`ckb_fixed_hash`], do not use this crate directly.**
//!
//! All proc-macros in this crate are re-exported in crate [`ckb_fixed_hash`].
//!
//! And you can found examples in crate [`ckb_fixed_hash`].
//!
//! [`FromStr::from_str`]: https://doc.rust-lang.org/std/str/trait.FromStr.html#tymethod.from_str
//! [`ckb_fixed_hash`]: ../ckb_fixed_hash/index.html

extern crate proc_macro;

use std::str::FromStr;

use quote::quote;
use syn::parse_macro_input;

macro_rules! impl_hack {
    ($name:ident, $type:ident, $type_str:expr, $link_str:expr) =>    {
        #[doc = "A proc-macro used to create a const [`"]
        #[doc = $type_str]
        #[doc = "`] from a hexadecimal string or a trimmed hexadecimal string.\n\n[`"]
        #[doc = $type_str]
        #[doc = "`]:"]
        #[doc = $link_str]
        #[proc_macro]
        pub fn $name(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
            let input = parse_macro_input!(input as syn::LitStr);
            let expanded = {
                let input = input.value().replace("_", "");
                if input.len() < 3 || &input[..2] != "0x" {
                    panic!("Input has to be a hexadecimal string with 0x-prefix.");
                };
                let input_str = &input[2..];
                let value = match &input_str[..1] {
                    "0" => {
                        if input_str.len() > 1 {
                            ckb_fixed_hash_core::$type::from_str(input_str)
                        } else {
                            ckb_fixed_hash_core::$type::from_trimmed_str(input_str)
                        }
                    },
                    _ => {
                        ckb_fixed_hash_core::$type::from_trimmed_str(input_str)
                    },
                }
                .unwrap_or_else(|err| {
                    panic!("Failed to parse the input hexadecimal string: {}", err);
                });
                let eval_str = format!("{:?}", value);
                let eval_ts: proc_macro2::TokenStream = eval_str.parse().unwrap_or_else(|_| {
                    panic!("Failed to parse the string \"{}\" to TokenStream.", eval_str);
                });
                quote!(#eval_ts)
            };
            expanded.into()
        }
    };
    ($name:ident, $type:ident) => {
        impl_hack!($name, $type, stringify!($type), concat!("../ckb_fixed_hash_core/struct.", stringify!($type), ".html"));
    }
}

impl_hack!(h160, H160);
impl_hack!(h256, H256);
impl_hack!(h512, H512);
impl_hack!(h520, H520);
