use std::str::FromStr;

use crate::{H160, H256, H512, H520};

macro_rules! impl_serde {
    ($name:ident, $bytes_size:expr) => {
        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let bytes = &self.0[..];
                let mut dst = [0u8; $bytes_size * 2 + 2];
                dst[0] = b'0';
                dst[1] = b'x';
                faster_hex::hex_encode(bytes, &mut dst[2..])
                    .map_err(|e| serde::ser::Error::custom(format!("{e}")))?;
                serializer.serialize_str(unsafe { ::std::str::from_utf8_unchecked(&dst) })
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct Visitor;

                impl<'b> serde::de::Visitor<'b> for Visitor {
                    type Value = $name;

                    fn expecting(
                        &self,
                        formatter: &mut ::std::fmt::Formatter,
                    ) -> ::std::fmt::Result {
                        write!(
                            formatter,
                            "a 0x-prefixed hex string with {} digits",
                            $bytes_size * 2
                        )
                    }

                    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        let b = v.as_bytes();
                        if b.len() <= 2 || &b[0..2] != b"0x" {
                            return Err(E::custom(format_args!(
                                "invalid format, expected {}",
                                &self as &dyn serde::de::Expected
                            )));
                        }

                        if b.len() != $bytes_size * 2 + 2 {
                            return Err(E::invalid_length(b.len() - 2, &self));
                        }

                        $name::from_str(&v[2..]).map_err(|e| {
                            E::custom(format_args!(
                                "invalid hex bytes: {:?}, expected {}",
                                e, &self as &dyn serde::de::Expected
                            ))
                        })
                    }

                    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        self.visit_str(&v)
                    }
                }
                deserializer.deserialize_str(Visitor)
            }
        }
    };
}

impl_serde!(H160, 20);
impl_serde!(H256, 32);
impl_serde!(H512, 64);
impl_serde!(H520, 65);
