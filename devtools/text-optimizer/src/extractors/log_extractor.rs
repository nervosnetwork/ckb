use super::{extract_contents_in_brackets, LogExtractor};
use crate::types::{Category, Meta, TextInfo};
use std::str::FromStr;
use syn::Macro;

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
                        let meta = Meta::new(category, self.file_path.to_owned(), start_line);
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
