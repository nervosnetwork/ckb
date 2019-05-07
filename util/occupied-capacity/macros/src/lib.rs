extern crate proc_macro;

use proc_macro2::TokenStream;
use proc_macro_hack::proc_macro_hack;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{
    parse_macro_input, parse_quote, Data, DeriveInput, Fields, GenericParam, Generics, Index,
};

use occupied_capacity_core::Capacity;

#[proc_macro_hack]
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

#[proc_macro_derive(HasOccupiedCapacity, attributes(free_capacity))]
pub fn derive_occupied_capacity(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(input as DeriveInput);

    // Used in the quasi-quotation below as `#name`.
    let name = input.ident;

    // Add a bound `T: OccupiedCapacity` to every type parameter T.
    let generics = add_trait_bounds(input.generics);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let sum = occupied_capacity_sum(&input.data);

    let expanded = quote! {
        // The generated impl.
        impl #impl_generics ::occupied_capacity::OccupiedCapacity for #name #ty_generics #where_clause {
            fn occupied_capacity(&self) -> ::occupied_capacity::Result<::occupied_capacity::Capacity> {
                #sum
            }
        }
    };

    // Hand the output tokens back to the compiler.
    proc_macro::TokenStream::from(expanded)
}

// Add a bound `T: OccupiedCapacity` to every type parameter T.
fn add_trait_bounds(mut generics: Generics) -> Generics {
    for param in &mut generics.params {
        if let GenericParam::Type(ref mut type_param) = *param {
            type_param
                .bounds
                .push(parse_quote!(::occupied_capacity::OccupiedCapacity));
        }
    }
    generics
}

fn has_free_capacity(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.parse_meta()
            .map(|meta| {
                if let syn::Meta::Word(ident) = meta {
                    ident == "free_capacity"
                } else {
                    false
                }
            })
            .unwrap_or(false)
    })
}

// Generate an expression to sum up the heap size of each field.
fn occupied_capacity_sum(data: &Data) -> TokenStream {
    match *data {
        Data::Struct(ref data) => {
            match data.fields {
                Fields::Named(ref fields) => {
                    // Expands to an expression like
                    //
                    //      Ok(::occupied_capacity::Capacity::zero())
                    //          .and_then(|x|
                    //              self.field0.occupied_capacity().and_then(|y| x.safe_add(y)))
                    //          .and_then(|x|
                    //              self.field1.occupied_capacity().and_then(|y| x.safe_add(y)))
                    //          ... ...
                    //
                    // but using fully qualified function call syntax.
                    //
                    // We take some care to use the span of each `syn::Field` as
                    // the span of the corresponding `occupied_capacity`
                    // call. This way if one of the field types does not
                    // implement `OccupiedCapacity` then the compiler's error message
                    // underlines which field it is.
                    let recurse = fields.named.iter().filter(|f| {
                        !has_free_capacity(&f.attrs[..])
                    }).map(|f| {
                        let name = &f.ident;
                        quote_spanned! {f.span()=>
                            ::occupied_capacity::OccupiedCapacity::occupied_capacity(&self.#name)
                        }
                    });
                    quote! {
                        Ok(::occupied_capacity::Capacity::zero())
                        #(
                            .and_then(|x| {
                                #recurse.and_then(|y| x.safe_add(y))
                            })
                        )*
                    }
                }
                Fields::Unnamed(ref fields) => {
                    // Expands to an expression like
                    //
                    //      Ok(::occupied_capacity::Capacity::zero())
                    //          .and_then(|x|
                    //              self.0.occupied_capacity().and_then(|y| x.safe_add(y)))
                    //          .and_then(|x|
                    //              self.1.occupied_capacity().and_then(|y| x.safe_add(y)))
                    //          ... ...
                    //
                    let recurse = fields.unnamed.iter().enumerate().filter(|(_,f)| {
                        !has_free_capacity(&f.attrs[..])
                    }).map(|(i, f)| {
                        let index = Index::from(i);
                        quote_spanned! {f.span()=>
                            ::occupied_capacity::OccupiedCapacity::occupied_capacity(&self.#index)
                        }
                    });
                    quote! {
                        Ok(::occupied_capacity::Capacity::zero())
                        #(
                            .and_then(|x| {
                                #recurse.and_then(|y| x.safe_add(y))
                            })
                        )*
                    }
                }
                Fields::Unit => {
                    // Unit structs cannot own more than 0 bytes of heap memory.
                    quote!(Ok(::occupied_capacity::Capacity::zero()))
                }
            }
        }
        Data::Enum(_) | Data::Union(_) => unimplemented!(),
    }
}
