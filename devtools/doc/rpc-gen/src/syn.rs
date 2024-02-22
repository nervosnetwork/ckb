//! It's bad(sad) JSON Schema currently ignore type alias,
//! maybe it's better to fix it in schemars, but here we only do a quick hack
//! here we use a simple syn visitor to find extra type comments

use std::collections::HashMap;
use syn::visit::Visit;
use syn::{parse2, Expr, ItemType, Meta, MetaNameValue};
use walkdir::WalkDir;

struct CommentFinder {
    // Store the comments here
    type_comments: HashMap<String, String>,
    current_type: Option<String>,
    types: Vec<String>,
}

impl Visit<'_> for CommentFinder {
    fn visit_attribute(&mut self, attr: &syn::Attribute) {
        if let Some(type_name) = &self.current_type {
            if attr.path().is_ident("doc") {
                if let Meta::NameValue(MetaNameValue {
                    value:
                        Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(lit),
                            ..
                        }),
                    ..
                }) = &attr.meta
                {
                    let lit = lit.value();
                    let current_type = type_name.clone();
                    *self
                        .type_comments
                        .entry(current_type)
                        .or_insert("".to_string()) += &format!("\n{}", lit.trim_start());
                }
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
}

fn visit_source_file(finder: &mut CommentFinder, file_path: &std::path::Path) {
    let code = std::fs::read_to_string(file_path).unwrap();
    if let Ok(tokens) = code.parse() {
        if let Ok(file) = parse2(tokens) {
            finder.visit_file(&file);
        }
    }
}

pub(crate) fn visit_for_types() -> Vec<(String, String)> {
    let mut finder = CommentFinder {
        type_comments: Default::default(),
        current_type: None,
        types: vec![],
    };
    let dir = "util/jsonrpc-types";
    for entry in WalkDir::new(dir).follow_links(true).into_iter() {
        match entry {
            Ok(ref e)
                if !e.file_name().to_string_lossy().starts_with('.')
                    && e.file_name().to_string_lossy().ends_with(".rs") =>
            {
                visit_source_file(&mut finder, e.path());
            }
            _ => (),
        }
    }
    finder
        .type_comments
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
