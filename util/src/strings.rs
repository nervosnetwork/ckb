use regex::Regex;

pub fn check_if_identifier_is_valid(ident: &str) -> Result<(), String> {
    const IDENT_PATTERN: &str = r#"^[0-9a-zA-Z_-]+$"#;
    if ident.is_empty() {
        return Err("the identifier shouldn't be empty".to_owned());
    }
    match Regex::new(IDENT_PATTERN) {
        Ok(re) => {
            if !re.is_match(&ident) {
                return Err(format!(
                    "invaild identifier \"{}\", the identifier pattern is \"{}\"",
                    ident, IDENT_PATTERN
                ));
            }
        }
        Err(err) => {
            return Err(format!(
                "invalid regular expression \"{}\": {}",
                IDENT_PATTERN, err
            ));
        }
    }
    Ok(())
}
