//! It's bad(sad) JSON Schema currently ignore type alias,
//! maybe it's better to fix it in schemars, but here we only do a quick hack
//! here we use a simple syn visitor to find extra type comments

use proc_macro2::TokenTree;
use std::collections::HashMap;
use syn::visit::Visit;
use syn::{Expr, ItemType, Meta, MetaNameValue, parse2};
use walkdir::WalkDir;

pub(crate) struct CommentFinder {
    // Store the comments here
    pub type_comments: HashMap<String, String>,
    pub deprecated: HashMap<String, (String, String)>,
    current_type: Option<String>,
    current_fn: Option<String>,
    types: Vec<String>,
}

fn get_deprected_attr(attr: &syn::Attribute) -> Option<Vec<String>> {
    if attr.path().is_ident("deprecated") {
        let mut vals = vec![];
        if let Meta::List(list) = &attr.meta {
            for token in list.tokens.clone().into_iter() {
                if let TokenTree::Literal(lit) = token {
                    let v = lit.to_string().trim_matches('\"').to_string();
                    vals.push(v);
                }
            }
        }
        if vals.len() == 2 {
            return Some(vals);
        }
    }
    None
}

fn get_doc_from_attr(attr: &syn::Attribute) -> String {
    if attr.path().is_ident("doc")
        && let Meta::NameValue(MetaNameValue {
            value:
                Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }),
            ..
        }) = &attr.meta
        {
            let lit = lit.value();
            return lit;
        }
    "".to_string()
}

impl Visit<'_> for CommentFinder {
    fn visit_attribute(&mut self, attr: &syn::Attribute) {
        if let Some(type_name) = &self.current_type {
            let doc = get_doc_from_attr(attr);
            let current_type = type_name.clone();
            *self
                .type_comments
                .entry(current_type)
                .or_insert("".to_string()) += &format!("\n{}", doc.trim_start());
        }
        if let Some(fn_name) = &self.current_fn {
            let deprecated = get_deprected_attr(attr);
            if let Some(ref vals) = deprecated {
                self.deprecated
                    .insert(fn_name.to_string(), (vals[0].clone(), vals[1].clone()));
            }
        }
    }

    fn visit_item_struct(&mut self, i: &syn::ItemStruct) {
        let ident_name = i.ident.to_string();
        if self.types.contains(&ident_name) && !i.attrs.is_empty() {
            self.current_type = Some(ident_name);
            for attr in &i.attrs {
                self.visit_attribute(attr);
            }
            self.current_type = None;
        }
    }

    fn visit_item_type(&mut self, i: &ItemType) {
        let ident_name = i.ident.to_string();
        if !i.attrs.is_empty() {
            self.current_type = Some(ident_name);
            for attr in &i.attrs {
                self.visit_attribute(attr);
            }
            self.current_type = None;
        }
    }

    fn visit_item_trait(&mut self, trait_item: &'_ syn::ItemTrait) {
        for i in trait_item.items.iter() {
            if let syn::TraitItem::Fn(item_fn) = i
                && !item_fn.attrs.is_empty() {
                    let current_rpc = trait_item
                        .ident
                        .to_string()
                        .trim_end_matches("Rpc")
                        .to_owned();
                    self.current_fn = Some(format!("{}.{}", current_rpc, item_fn.sig.ident));
                    for attr in &item_fn.attrs {
                        self.visit_attribute(attr);
                    }
                    self.current_fn = None;
                }
        }
    }

    fn visit_item_enum(&mut self, i: &'_ syn::ItemEnum) {
        let ident_name = i.ident.to_string();
        if self.types.contains(&ident_name) {
            if !i.attrs.is_empty() {
                self.current_type = Some(ident_name);
                for attr in &i.attrs {
                    self.visit_attribute(attr);
                }
                self.current_type = None;
            }
            let mut variants = vec![];
            for v in &i.variants {
                if !v.attrs.is_empty() {
                    let doc: Vec<String> = v.attrs.iter().map(get_doc_from_attr).collect();
                    let doc = doc.join("\n");
                    variants.push(format!("  - `{}` : {}", v.ident, doc));
                }
            }
            let extra_doc = variants.join("\n");
            *self
                .type_comments
                .entry(i.ident.to_string())
                .or_insert("".to_string()) += &format!("An enum value from one of:\n{}", extra_doc);
        }
    }
}

impl CommentFinder {
    fn visit_source_file(&mut self, file_path: &std::path::Path) {
        let code = std::fs::read_to_string(file_path).unwrap();
        if let Ok(tokens) = code.parse()
            && let Ok(file) = parse2(tokens) {
                self.visit_file(&file);
            }
    }

    pub fn new() -> CommentFinder {
        let mut finder = CommentFinder {
            type_comments: Default::default(),
            current_type: None,
            current_fn: None,
            types: ["JsonBytes", "IndexerRange", "PoolTransactionReject"]
                .iter()
                .map(|&s| s.to_owned())
                .collect(),
            deprecated: Default::default(),
        };
        for dir in ["util/jsonrpc-types", "rpc/src/module"] {
            for entry in WalkDir::new(dir).follow_links(true).into_iter() {
                match entry {
                    Ok(ref e)
                        if !e.file_name().to_string_lossy().starts_with('.')
                            && e.file_name().to_string_lossy().ends_with(".rs") =>
                    {
                        finder.visit_source_file(e.path());
                    }
                    _ => (),
                }
            }
        }
        finder
    }
}
