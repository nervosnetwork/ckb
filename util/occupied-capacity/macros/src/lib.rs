//! TODO(doc): @keroro520
extern crate proc_macro;

use quote::quote;
use syn::{parse_macro_input, Error as SynError};

use ckb_occupied_capacity_core::Capacity;

/// TODO(doc): @keroro520
#[proc_macro]
pub fn capacity_bytes(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as syn::LitInt);
    let expanded = {
        if input.suffix().is_empty() {
            input
                .base10_parse::<usize>()
                .map_err(|_| {
                    SynError::new(
                        input.span(),
                        "the input should be a positive integer literal",
                    )
                })
                .and_then(|value| {
                    Capacity::bytes(value)
                        .map_err(|_| SynError::new(input.span(), "the input capacity is overflow"))
                        .map(|value| {
                            let shannons =
                                syn::LitInt::new(&value.as_u64().to_string(), input.span());
                            quote!(Capacity::shannons(#shannons))
                        })
                })
        } else {
            Err(SynError::new(
                input.span(),
                "the input should be an integer literal without any suffix",
            ))
        }
        .unwrap_or_else(|err| err.to_compile_error())
    };
    expanded.into()
}
