//! TODO(doc): @yangby-cryptape

extern crate proc_macro;

use std::str::FromStr;

use quote::quote;
use syn::parse_macro_input;

macro_rules! impl_hack {
    ($name:ident, $type:ident) =>    {
        /// TODO(doc): @yangby-cryptape
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
}

impl_hack!(h160, H160);
impl_hack!(h256, H256);
impl_hack!(h512, H512);
impl_hack!(h520, H520);
