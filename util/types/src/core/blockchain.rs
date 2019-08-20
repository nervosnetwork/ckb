use failure::{err_msg, Error as FailureError};
use std::convert::TryFrom;

// NOTE: we could've used enum as well in the wire format, but as of
// flatbuffer 1.11.0, unused constants will be generated in the Rust
// code for enum types, resulting in both compiler warnings and clippy
// errors. So for now we are sticking to a single integer in the wire
// format, and only use enums in core data structures.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptHashType {
    Data = 0,
    Type = 1,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data
    }
}

impl TryFrom<u8> for ScriptHashType {
    type Error = FailureError;

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(ScriptHashType::Data),
            1 => Ok(ScriptHashType::Type),
            _ => Err(err_msg(format!("Invalid string hash type {}", v))),
        }
    }
}
