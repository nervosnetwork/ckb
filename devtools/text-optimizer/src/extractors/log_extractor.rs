use super::{extract_contents_in_brackets, Extractor};
use crate::types::{Category, Meta, TextInfo};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use syn::Macro;
use syn::{visit::visit_file, File};

#[derive(Default)]
pub struct LogExtractor {
    save_file: PathBuf,
    map: HashMap<(String, PathBuf), TextInfo>,
    scanning_file_path: PathBuf,
}

impl LogExtractor {
    pub fn new(save_file: PathBuf) -> Self {
        LogExtractor {
            save_file,
            ..Default::default()
        }
    }
}

impl Extractor for LogExtractor {
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

impl syn::visit::Visit<'_> for LogExtractor {
    fn visit_macro(&mut self, node: &Macro) {
        if let Some(name) = get_macro_name(node) {
            if name == "error"
                || name == "warn"
                || name == "info"
                || name == "debug"
                || name == "trace"
            {
                if let Some(lit) = node.tokens.clone().into_iter().next() {
                    if let Some(text) = extract_contents_in_brackets(lit.to_string()) {
                        log::trace!("Found target text: {}", text);

                        let span = lit.span();
                        let start_line = span.start().line;
                        let category = Category::from_str(&name).unwrap();
                        let meta =
                            Meta::new(category, self.scanning_file_path.to_owned(), start_line);
                        self.add_text_info(TextInfo::new(text, meta));
                    }
                }
            }
        }
    }
}

fn get_macro_name(node: &Macro) -> Option<String> {
    if let Some(ident) = node.path.get_ident() {
        Some(ident.to_string())
    } else {
        node.path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
    }
}
