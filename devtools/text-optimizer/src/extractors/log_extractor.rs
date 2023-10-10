use super::{extract_contents_in_brackets, LogExtractor};
use crate::types::{Category, Meta, TextInfo};
use std::str::FromStr;
use syn::Macro;

impl syn::visit::Visit<'_> for LogExtractor {
    fn visit_macro(&mut self, node: &Macro) {
        if let Some(ident) = node.path.get_ident() {
            // Determine if the macro is println!
            if ident == "error"
                || ident == "warn"
                || ident == "info"
                || ident == "debug"
                || ident == "trace"
            {
                if let Some(open_paren) = node.tokens.clone().into_iter().next() {
                    if let Some(text) = extract_contents_in_brackets(open_paren.to_string()) {
                        println!("Found format string: {}", text);

                        let span = open_paren.span();
                        let start_line = span.start().line;
                        let end_line = span.end().line;
                        let category = Category::from_str(ident.to_string().as_str()).unwrap();
                        let meta =
                            Meta::new(category, self.file_path.to_owned(), start_line, end_line);
                        self.add_text_info(TextInfo::new(text, meta));
                    }
                }
            }
        }
    }
}
