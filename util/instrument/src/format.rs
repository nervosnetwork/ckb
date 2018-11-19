use std::fmt;
use std::str::FromStr;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Format {
    Json,
    Binary,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Format::Json => write!(f, "json"),
            Format::Binary => write!(f, "bin"),
        }
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bin" => Ok(Format::Binary),
            "json" => Ok(Format::Json),
            format => Err(format!("Unsupported format: {}", format)),
        }
    }
}
