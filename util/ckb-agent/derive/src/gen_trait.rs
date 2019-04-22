use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Result, Error};

pub fn crate_name(name: &str) -> Result<Ident> {
	proc_macro_crate::crate_name(name)
		.map(|name| Ident::new(&name, Span::call_site()))
		.map_err(|e| Error::new(Span::call_site(), &e))
}

pub fn gen_impl(input: syn::Item) -> Result<proc_macro2::TokenStream> {
	let agent_trait = match input {
		syn::Item::Trait(item_trait) => item_trait,
		item => {
			return Err(syn::Error::new_spanned(
				item,
				"The #[agent] custom attribute only works with trait declarations",
			));
		}
	};

	let trait_methods = compute_methods(&agent_trait)?;

	let name = agent_trait.ident.clone();
	let mod_name_ident = wrapper_mod_name(&agent_trait);

	let agent_mod = crate_name("ckb-agent")?;

    let controller = gen_controller();
    let agent_start_method = gen_controller();
	Ok(quote!(
		mod #mod_name_ident {
			use #agent_mod as _agent_mod;
			use super::*;

            #controller

            //receiver
            //start_trait
		}
		//#(#exports)*
	))
}

pub fn gen_controller(methods: &[u32], item_trait: &syn::ItemTrait) -> Result<TokenStream> {
	let client_methods = gen_controller_methods(methods)?;
	let client_senders = {
        let senders = vec![];
        for method in methods {
            let method_name = method.name();
            let sender_name = format!("{}_sender", method_name);
            let args = {};
            let returns = {};
            let method_sender = syn::parse_quote! {
                #sender_name: Sender<Request<#args, #returns>>
            };
        }
    };
	let generics = &item_trait.generics;
	Ok(quote! {
        pub struct #controller_name {
            #(#client_senders,)*
            stop: StopHandler<()>,
        }

        impl #controller_name {
            #(#client_methods)*
        }
	})
}

fn gen_controller_methods(methods: &[MethodRegistration]) -> Result<Vec<syn::ImplItem>> {
	let mut client_methods = vec![];
    for method in methods {
        let method_name = method.name();
        let name = &method.trait_item.sig.ident;
        let args = compute_args(&method.trait_item);
        let arg_names = compute_arg_identifiers(&args)?;
        let returns = compute_returns(&method.trait_item, returns)?;
        let returns_str = quote!(#returns).to_string();
        let client_method = syn::parse_quote! {
            pub fn #name(&self, #args) -> #returns {
                let args_tuple = (#(#arg_names,)*);
                Request::call(&self.#sender, args_tuple).expect("#method_name() failed")
            }
        };
        client_methods.push(client_method);
    }
    Ok(client_methods)
}

fn wrapper_mod_name(rpc_trait: &syn::ItemTrait) -> syn::Ident {
    let name = rpc_trait.ident.clone();
    let mod_name = format!("{}{}", MOD_NAME_PREFIX, name.to_string());
    syn::Ident::new(&mod_name, proc_macro2::Span::call_site())
}

fn compute_args(method: &syn::TraitItemMethod) -> Punctuated<syn::FnArg, syn::token::Comma> {
	let mut args = Punctuated::new();
	for arg in &method.sig.decl.inputs {
		let ty = match arg {
			syn::FnArg::Captured(syn::ArgCaptured { ty, .. }) => ty,
			_ => continue,
		};
		let segments = match ty {
			syn::Type::Path(syn::TypePath {
				path: syn::Path { segments, .. },
				..
			}) => segments,
			_ => continue,
		};
		let ident = match &segments[0] {
			syn::PathSegment { ident, .. } => ident,
		};
		if ident.to_string() == "Self" {
			continue;
		}
		args.push(arg.to_owned());
	}
	args
}

fn compute_returns(method: &syn::TraitItemMethod, returns: &Option<String>) -> Result<syn::Type> {
	let returns: Option<syn::Type> = match returns {
		Some(returns) => Some(syn::parse_str(returns)?),
		None => None,
	};
	let returns = match returns {
		None => try_infer_returns(&method.sig.decl.output),
		_ => returns,
	};
	let returns = match returns {
		Some(returns) => returns,
		None => {
			let span = method.attrs[0].pound_token.spans[0];
			let msg = "Missing returns attribute.";
			return Err(syn::Error::new(span, msg));
		}
	};
	Ok(returns)
}

fn try_infer_returns(output: &syn::ReturnType) -> Option<syn::Type> {
	match output {
		syn::ReturnType::Type(_, ty) => match &**ty {
			syn::Type::Path(syn::TypePath {
				path: syn::Path { segments, .. },
				..
			}) => match &segments[0] {
				syn::PathSegment { ident, arguments, .. } => {
					if ident.to_string().ends_with("Result") {
						get_first_type_argument(arguments)
					} else {
						None
					}
				}
			},
			_ => None,
		},
		_ => None,
	}
}

fn get_first_type_argument(args: &syn::PathArguments) -> Option<syn::Type> {
	match args {
		syn::PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments { args, .. }) => {
			if args.len() > 0 {
				match &args[0] {
					syn::GenericArgument::Type(ty) => Some(ty.to_owned()),
					_ => None,
				}
			} else {
				None
			}
		}
		_ => None,
	}
}

fn compute_methods(item_trait: &syn::ItemTrait) -> Result<Vec<RpcMethod>> {
    let methods_result: Result<Vec<_>> = item_trait
        .items
        .iter()
        .filter_map(|trait_item| {
            if let syn::TraitItem::Method(method) = trait_item {
                Some(method.clone())
            } else {
                None
            }
        })
    .collect();
    methods_result.into()
}



