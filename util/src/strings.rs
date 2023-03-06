//! Utilities for std strings.
use regex::Regex;

/// Checks whether the given string is a valid identifier.
///
/// This function considers non-empty string containing only alphabets, digits, `-`, and `_` as
/// a valid identifier.
///
/// ## Examples
///
/// ```
/// use ckb_util::strings::check_if_identifier_is_valid;
///
/// assert!(check_if_identifier_is_valid("test123").is_ok());
/// assert!(check_if_identifier_is_valid("123test").is_ok());
/// assert!(check_if_identifier_is_valid("").is_err());
/// assert!(check_if_identifier_is_valid("test 123").is_err());
/// ```
pub fn check_if_identifier_is_valid(ident: &str) -> Result<(), String> {
    const IDENT_PATTERN: &str = r#"^[0-9a-zA-Z_-]+$"#;
    static RE: once_cell::sync::OnceCell<Regex> = once_cell::sync::OnceCell::new();
    // IDENT_PATTERN is a correct regular expression, so unwrap here
    let re = RE.get_or_init(|| Regex::new(IDENT_PATTERN).unwrap());

    if ident.is_empty() {
        return Err("the identifier shouldn't be empty".to_owned());
    }
    if !re.is_match(ident) {
        return Err(format!(
            "Invalid identifier \"{ident}\", the identifier pattern is \"{IDENT_PATTERN}\""
        ));
    }
    Ok(())
}
