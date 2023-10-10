use super::{extract_contents_in_brackets, ThiserrorExtractor};
use crate::types::{Category, Meta, TextInfo};
use syn::Expr::{self, Lit};
use syn::Lit::Str;

impl syn::visit::Visit<'_> for ThiserrorExtractor {
    fn visit_attribute(&mut self, attr: &syn::Attribute) {
        if attr.path().is_ident("error") {
            let precondition: Expr = attr.parse_args().unwrap();
            if let Lit(lit) = precondition {
                if let Str(lit_str) = lit.lit {
                    let lit = lit_str.token().to_string();

                    if let Some(text) = extract_contents_in_brackets(lit) {
                        println!("Found format string: {}", text);

                        let span = lit_str.span();
                        let start_line = span.start().line;
                        let end_line = span.end().line;
                        let meta = Meta::new(
                            Category::ThisError,
                            self.file_path.to_owned(),
                            start_line,
                            end_line,
                        );
                        self.add_text_info(TextInfo::new(text, meta));
                    }
                }
            }
        }
    }
}
