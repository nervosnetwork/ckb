use std::{fmt, iter::Iterator, str::FromStr};

use crate::{utilities, Error};

#[derive(Debug, Clone)]
pub struct Arguments {
    original: String,
    pub(crate) items: Vec<String>,
}

impl fmt::Display for Arguments {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Arguments: [")?;
        if !self.items.is_empty() {
            write!(f, "{:?}", self.items[0])?;
            for item in &self.items[1..] {
                write!(f, ", {:?}", item)?;
            }
        }
        write!(f, "]")
    }
}

impl Arguments {
    pub fn iter(&self) -> &[String] {
        &self.items[..]
    }
}

impl FromStr for Arguments {
    type Err = Error;
    fn from_str(raw_input: &str) -> Result<Self, Self::Err> {
        let input = raw_input.trim();
        let mut items = Vec::new();
        if !input.is_empty() {
            let mut chars = input.chars().peekable();
            'has_char: while let Some(mut ch) = chars.next() {
                while ch.is_ascii_whitespace() {
                    if let Some(next) = chars.next() {
                        ch = next;
                    } else {
                        break 'has_char;
                    }
                }
                let item = match ch {
                    '"' | '\'' => utilities::find_quoted_string(ch, &mut chars)?,
                    _ => utilities::find_unquoted_string(ch, &mut chars),
                };
                items.push(item);
            }
        }
        Ok(Self {
            original: raw_input.to_owned(),
            items,
        })
    }
}
