//! TODO(doc): @keroro520
extern crate proc_macro;

use quote::quote;
use syn::parse_macro_input;

use ckb_occupied_capacity_core::Capacity;

/// TODO(doc): @keroro520
#[proc_macro]
pub fn capacity_bytes(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as syn::LitInt);
    let expanded = if let Ok(oc) = Capacity::bytes(input.value() as usize) {
        let shannons = syn::LitInt::new(oc.as_u64(), syn::IntSuffix::None, input.span());
        quote!(Capacity::shannons(#shannons))
    } else {
        panic!("Occupied capacity is overflow.");
    };
    expanded.into()
}
