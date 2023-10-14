use super::{extract_contents_in_brackets, Extractor};
use crate::types::{Category, Meta, TextInfo};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::Expr::{self, Lit};
use syn::Lit::Str;
use syn::{visit::visit_file, File};

#[derive(Default)]
pub struct ThiserrorExtractor {
    save_file: PathBuf,
    map: HashMap<(String, PathBuf), TextInfo>,
    scanning_file_path: PathBuf,
}

impl ThiserrorExtractor {
    pub fn new(save_file: PathBuf) -> Self {
        ThiserrorExtractor {
            save_file,
            ..Default::default()
        }
    }
}

impl Extractor for ThiserrorExtractor {
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
        self.map.values().cloned().collect()
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

impl syn::visit::Visit<'_> for ThiserrorExtractor {
    fn visit_attribute(&mut self, attr: &syn::Attribute) {
        if attr.path().is_ident("error") {
            let precondition: Expr = {
                if let Ok(precondition) = attr.parse_args() {
                    precondition
                } else {
                    let span = attr.span();
                    log::warn!(
                        "parse args failed, ignore the file: {:?}, code line: {:?}",
                        self.scanning_file_path,
                        span.start().line
                    );
                    return;
                }
            };
            if let Lit(lit) = precondition {
                if let Str(lit_str) = lit.lit {
                    let lit = lit_str.token().to_string();

                    if let Some(text) = extract_contents_in_brackets(lit) {
                        log::trace!("Found target text: {}", text);

                        let span = lit_str.span();
                        let start_line = span.start().line;
                        let meta = Meta::new(
                            Category::ThisError,
                            self.scanning_file_path.to_owned(),
                            start_line,
                        );
                        self.add_text_info(TextInfo::new(text, meta));
                    }
                }
            }
        }
    }
}
