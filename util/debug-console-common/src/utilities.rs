use std::{
    iter::{Iterator, Peekable},
    str::Chars,
};

use crate::Error;

pub(crate) fn find_quoted_string(ch: char, chars: &mut Peekable<Chars>) -> Result<String, Error> {
    let mut item = String::new();
    let mut matched = false;
    if ch == '"' {
        let mut escape = false;
        while let Some(ch2) = chars.next() {
            if ch2 == ch {
                if escape {
                    item.push(ch2);
                    escape = false;
                } else {
                    matched = true;
                    break;
                }
            } else if ch2 == '\\' {
                if escape {
                    item.push(ch2);
                    escape = false;
                } else {
                    escape = true;
                }
            } else if escape {
                return Err(Error::InvalidEscape);
            } else {
                item.push(ch2);
            }
        }
    } else {
        while let Some(ch2) = chars.next() {
            if ch2 == ch {
                matched = true;
                break;
            } else {
                item.push(ch2);
            }
        }
    }
    if !matched {
        return Err(Error::UnmatchedQuotes);
    }
    if let Some(ch2) = chars.next() {
        if !ch2.is_ascii_whitespace() {
            return Err(Error::IllegalCharacter);
        }
    }
    Ok(item)
}

pub(crate) fn find_unquoted_string(ch: char, chars: &mut Peekable<Chars>) -> String {
    let mut item = String::new();
    item.push(ch);
    for ch2 in chars {
        if ch2.is_ascii_whitespace() {
            break;
        } else {
            item.push(ch2);
        }
    }
    item
}
