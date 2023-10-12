use super::{extract_contents_in_brackets, LogExtractor};
use crate::types::{Category, Meta, TextInfo};
use std::str::FromStr;
use syn::Macro;

impl syn::visit::Visit<'_> for LogExtractor {
    fn visit_macro(&mut self, node: &Macro) {
        if let Some(ident) = node.path.get_ident() {
            if ident == "error"
                || ident == "warn"
                || ident == "info"
                || ident == "debug"
                || ident == "trace"
            {
                if let Some(lit) = node.tokens.clone().into_iter().next() {
                    if let Some(text) = extract_contents_in_brackets(lit.to_string()) {
                        log::trace!("Found target text: {}", text);

                        let span = lit.span();
                        let start_line = span.start().line;
                        let category = Category::from_str(ident.to_string().as_str()).unwrap();
                        let meta = Meta::new(category, self.file_path.to_owned(), start_line);
                        self.add_text_info(TextInfo::new(text, meta));
                    }
                }
            }
        }
    }
}
