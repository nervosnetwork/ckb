use super::{extract_contents_in_brackets, ClapExtractor};
use crate::types::{Category, Meta, TextInfo};
use std::str::FromStr;
use syn::Expr::{self};
use syn::Lit::Str;

impl syn::visit::Visit<'_> for ClapExtractor {
    fn visit_expr_method_call(&mut self, expr: &syn::ExprMethodCall) {
        let method_ident = &expr.method;
        if *method_ident == "about" || *method_ident == "help" {
            if let Some(Expr::Lit(lit)) = expr.args.first() {
                if let Str(lit_str) = &lit.lit {
                    let lit = lit_str.token().to_string();

                    if let Some(text) = extract_contents_in_brackets(lit) {
                        println!("Found format string: {}", text);

                        let span = lit_str.span();
                        let start_line = span.start().line;
                        let end_line = span.end().line;
                        let category =
                            Category::from_str(method_ident.to_string().as_str()).unwrap();
                        let meta =
                            Meta::new(category, self.file_path.to_owned(), start_line, end_line);
                        self.add_text_info(TextInfo::new(text, meta));
                    }
                }
            }
        }
        syn::visit::visit_expr_method_call(self, expr);
    }
}
