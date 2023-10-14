use super::{extract_contents_in_brackets, Extractor};
use crate::types::{Category, Meta, TextInfo};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use syn::Expr::{self};
use syn::Lit::Str;
use syn::{visit::visit_file, File};

#[derive(Default)]
pub struct ClapExtractor {
    save_file: PathBuf,
    map: HashMap<(String, PathBuf), TextInfo>,
    scanning_file_path: PathBuf,
}

impl ClapExtractor {
    pub fn new(save_file: PathBuf) -> Self {
        ClapExtractor {
            save_file,
            ..Default::default()
        }
    }
}

impl Extractor for ClapExtractor {
    fn reset_scanning_path(&mut self, file_path: &Path) {
        self.scanning_file_path = file_path.to_owned();
    }

    fn add_text_info(&mut self, text_info: TextInfo) {
        let key = text_info.original().to_owned();
        let file = text_info.metadata().file().clone();

        if let Some(existing) = self.map.get_mut(&(key.to_owned(), file.clone())) {
            existing.append_new_line(text_info.metadata().start_lines()[0]);
        } else {
            self.map.insert((key, file), text_info);
        }
    }

    fn text_list(&self) -> Vec<TextInfo> {
        let mut text_list: Vec<TextInfo> = self.map.values().cloned().collect();
        text_list.sort_by(|a, b| {
            let cmp = a.metadata().file().cmp(b.metadata().file());
            if cmp == std::cmp::Ordering::Equal {
                a.metadata().start_lines()[0].cmp(&b.metadata().start_lines()[0])
            } else {
                cmp
            }
        });
        text_list
    }

    fn scanning_file_path(&self) -> &PathBuf {
        &self.scanning_file_path
    }

    fn save_file_path(&self) -> &PathBuf {
        &self.save_file
    }

    fn visit_file(&mut self, node: &File) {
        visit_file(self, node)
    }
}

impl syn::visit::Visit<'_> for ClapExtractor {
    fn visit_expr_method_call(&mut self, expr: &syn::ExprMethodCall) {
        let method_ident = &expr.method;
        if *method_ident == "about" || *method_ident == "help" {
            if let Some(Expr::Lit(lit)) = expr.args.first() {
                if let Str(lit_str) = &lit.lit {
                    let lit = lit_str.token().to_string();

                    if let Some(text) = extract_contents_in_brackets(lit) {
                        log::trace!("Found target text: {}", text);

                        let span = lit_str.span();
                        let start_line = span.start().line;
                        let category =
                            Category::from_str(method_ident.to_string().as_str()).unwrap();
                        let meta =
                            Meta::new(category, self.scanning_file_path.to_owned(), start_line);
                        self.add_text_info(TextInfo::new(text, meta));
                    }
                }
            }
        }
        syn::visit::visit_expr_method_call(self, expr);
    }
}
