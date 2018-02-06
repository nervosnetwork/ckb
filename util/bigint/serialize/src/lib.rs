extern crate rustc_hex;
extern crate serde;

use std::fmt;

use rustc_hex::{FromHex, ToHex};
use serde::{de, Deserializer, Serializer};

/// Serializes a slice of bytes.
pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let hex = ToHex::to_hex(bytes);
    serializer.serialize_str(&format!("0x{}", hex))
}

/// Serialize a slice of bytes as uint.
///
/// The representation will have all leading zeros trimmed.
pub fn serialize_uint<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let non_zero = bytes.iter().take_while(|b| **b == 0).count();
    let bytes = &bytes[non_zero..];
    if bytes.is_empty() {
        return serializer.serialize_str("0x0");
    }

    let hex = ToHex::to_hex(bytes);
    let has_leading_zero = !hex.is_empty() && &hex[0..1] == "0";
    serializer.serialize_str(&format!(
        "0x{}",
        if has_leading_zero { &hex[1..] } else { &hex }
    ))
}

/// Expected length of bytes vector.
#[derive(Debug, PartialEq, Eq)]
pub enum ExpectedLen {
    /// Any length in bytes.
    Any,
    /// Exact length in bytes.
    Exact(usize),
    /// A bytes length between (min; max].
    Between(usize, usize),
}

impl fmt::Display for ExpectedLen {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ExpectedLen::Any => write!(fmt, "even length"),
            ExpectedLen::Exact(v) => write!(fmt, "length of {}", v * 2),
            ExpectedLen::Between(min, max) => {
                write!(fmt, "length between ({}; {}]", min * 2, max * 2)
            }
        }
    }
}

/// Deserialize into vector of bytes.
pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_check_len(deserializer, ExpectedLen::Any)
}

/// Deserialize into vector of bytes with additional size check.
pub fn deserialize_check_len<'de, D>(deserializer: D, len: ExpectedLen) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Visitor {
        len: ExpectedLen,
    }

    impl<'a> de::Visitor<'a> for Visitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a 0x-prefixed hex string with {}", self.len)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            if v.len() < 2 || &v[0..2] != "0x" {
                return Err(E::custom("prefix is missing"));
            }

            let is_len_valid = match self.len {
                // just make sure that we have all nibbles
                ExpectedLen::Any => v.len() % 2 == 0,
                ExpectedLen::Exact(len) => v.len() == 2 * len + 2,
                ExpectedLen::Between(min, max) => v.len() <= 2 * max + 2 && v.len() > 2 * min + 2,
            };

            if !is_len_valid {
                return Err(E::invalid_length(v.len() - 2, &self));
            }

            let bytes = match self.len {
                ExpectedLen::Between(..) if v.len() % 2 != 0 => {
                    FromHex::from_hex(&*format!("0{}", &v[2..]))
                }
                _ => FromHex::from_hex(&v[2..]),
            };

            bytes.map_err(|e| E::custom(&format!("invalid hex value: {:?}", e)))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            self.visit_str(&v)
        }
    }
    // TODO [ToDr] Use raw bytes if we switch to RLP / binencoding
    // (visit_bytes, visit_bytes_buf)
    deserializer.deserialize_str(Visitor { len })
}
