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
