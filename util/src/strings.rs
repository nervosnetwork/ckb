//! Utilities for std strings.

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
    if ident.is_empty() {
        return Err("the identifier shouldn't be empty".to_owned());
    }
    if ident
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(format!(
            "Invalid identifier \"{ident}\", the identifier can only contain alphabets, digits, `-`, and `_`"
        ))
    }
}
