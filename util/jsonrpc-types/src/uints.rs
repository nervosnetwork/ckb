use ckb_types::{
    core, packed,
    prelude::{Pack, Unpack},
};
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{fmt, marker, num};

pub trait Uint: Copy + fmt::LowerHex {
    const NAME: &'static str;

    fn from_str_radix(src: &str, radix: u32) -> Result<Self, num::ParseIntError>;
}

#[derive(Copy, Clone, Default, PartialEq, PartialOrd, Ord, Eq, Hash, Debug)]
pub struct JsonUint<T: Uint>(T);

struct JsonUintVisitor<T: Uint>(marker::PhantomData<T>);

impl<T: Uint> JsonUint<T> {
    pub fn value(self) -> T {
        self.0
    }
}

impl<T: Uint> fmt::Display for JsonUint<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "0x{:x}", self.value())
    }
}

impl<T: Uint> From<T> for JsonUint<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T: Uint> Serialize for JsonUint<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<T: Uint> JsonUintVisitor<T> {
    #[inline]
    fn expecting(formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a hex-encoded, 0x-prefixed {}", T::NAME)
    }

    #[inline]
    fn visit_str<E>(value: &str) -> Result<JsonUint<T>, E>
    where
        E: Error,
    {
        let value_bytes = value.as_bytes();
        if value_bytes.len() < 3 || &value_bytes[..2] != b"0x" {
            return Err(Error::custom(format!(
                "Invalid {} {}: without `0x` prefix",
                T::NAME,
                value
            )));
        }
        if value_bytes[2] == b'0' && value_bytes.len() > 3 {
            return Err(Error::custom(format!(
                "Invalid {} {}: with redundant leading zeros",
                T::NAME,
                value,
            )));
        }

        let number = T::from_str_radix(&value[2..], 16)
            .map(JsonUint)
            .map_err(|e| Error::custom(format!("Invalid {} {}: {}", T::NAME, value, e)))?;
        if number.to_string() != value {
            return Err(Error::custom(format!(
                "Invalid {} {}: only digits and lowercases are allowed",
                T::NAME,
                value,
            )));
        }

        Ok(number)
    }
}

macro_rules! def_json_uint {
    ($alias:ident, $inner:ident, $bits:expr) => {
        #[doc = "The "]
        #[doc = $bits]
        #[doc = r#" unsigned integer type encoded as the 0x-prefixed hex string in JSON.

## Examples

| JSON   | Decimal Value                |
| -------| ---------------------------- |
| "0x0"  | 0                            |
| "0x10" | 16                           |
| "10"   | Invalid, 0x is required      |
| "0x01" | Invalid, redundant leading 0 |"#]
        pub type $alias = JsonUint<$inner>;

        impl Uint for $inner {
            const NAME: &'static str = stringify!($alias);

            fn from_str_radix(src: &str, radix: u32) -> Result<Self, num::ParseIntError> {
                $inner::from_str_radix(src, radix)
            }
        }

        impl From<JsonUint<$inner>> for $inner {
            fn from(value: JsonUint<$inner>) -> Self {
                value.value()
            }
        }
    };
}

// TODO I tried `JsonUintVisitor<T>(PhantomData<T>)`, but `serde::Deserializer` doesn't supported.
macro_rules! impl_serde_deserialize {
    ($visitor:ident, $inner:ident) => {
        impl<'a> Deserialize<'a> for JsonUint<$inner> {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'a>,
            {
                deserializer.deserialize_str($visitor)
            }
        }

        struct $visitor;

        impl<'a> Visitor<'a> for $visitor {
            type Value = JsonUint<$inner>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                JsonUintVisitor::<$inner>::expecting(formatter)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                JsonUintVisitor::<$inner>::visit_str(value)
            }
        }
    };
}

macro_rules! impl_pack_and_unpack {
    ($packed:ident, $inner:ident) => {
        impl Pack<packed::$packed> for JsonUint<$inner> {
            fn pack(&self) -> packed::$packed {
                self.value().pack()
            }
        }

        impl Unpack<JsonUint<$inner>> for packed::$packed {
            fn unpack(&self) -> JsonUint<$inner> {
                Unpack::<$inner>::unpack(self).into()
            }
        }
    };
}

def_json_uint!(Uint32, u32, "32-bit");
def_json_uint!(Uint64, u64, "64-bit");
def_json_uint!(Uint128, u128, "128-bit");
impl_serde_deserialize!(Uint32Visitor, u32);
impl_serde_deserialize!(Uint64Visitor, u64);
impl_serde_deserialize!(Uint128Visitor, u128);
impl_pack_and_unpack!(Uint32, u32);
impl_pack_and_unpack!(Uint64, u64);
impl_pack_and_unpack!(Uint128, u128);

impl From<core::Capacity> for JsonUint<u64> {
    fn from(value: core::Capacity) -> Self {
        JsonUint(value.as_u64())
    }
}

impl From<core::EpochNumberWithFraction> for JsonUint<u64> {
    fn from(value: core::EpochNumberWithFraction) -> Self {
        JsonUint(value.full_value())
    }
}

impl From<JsonUint<u64>> for core::Capacity {
    fn from(value: JsonUint<u64>) -> Self {
        core::Capacity::shannons(value.value())
    }
}

#[cfg(tests)]
mod tests {
    macro_rules! test_json_uint {
        ($tests_name:ident, $name:ident, $inner:ident) => {
            mod $tests_name {
                use crate::$name;

                #[test]
                fn serialize() {
                    assert_eq!(r#""0xd""#, serde_json::to_string(&$name::from(13)).unwrap());
                    assert_eq!(r#""0x0""#, serde_json::to_string(&$name::from(0)).unwrap());
                }

                #[test]
                fn deserialize_heximal() {
                    let s = r#""0xa""#;
                    let deserialized: $name = serde_json::from_str(s).unwrap();
                    assert_eq!(deserialized, $name::from(10));
                }

                #[test]
                fn deserialize_decimal() {
                    let s = r#""10""#;
                    let ret: Result<$name, _> = serde_json::from_str(s);
                    assert!(ret.is_err(), ret);
                }

                #[test]
                fn deserialize_with_redundant_leading_zeros() {
                    let cases = vec![r#""0x01""#, r#""0x00""#];
                    for s in cases {
                        let ret: Result<$name, _> = serde_json::from_str(s);
                        assert!(ret.is_err(), ret);

                        let err = ret.unwrap_err();
                        assert!(
                            err.to_string().contains("with redundant leading zeros"),
                            err,
                        );
                    }
                }

                fn deserialize_with_uppercases() {
                    let cases = vec![r#""0xFF""#, r#""0xfF""#];
                    for s in cases {
                        let ret: Result<$name, _> = serde_json::from_str(s);
                        assert!(ret.is_err(), ret);

                        let err = ret.unwrap_err();
                        assert!(
                            err.to_string()
                                .contains("only digits and lowercases are allowed"),
                            err,
                        );
                    }
                }

                #[test]
                fn deserialize_too_large() {
                    let s = format!(r#""0x{:x}0""#, $inner::max_value());
                    let ret: Result<$name, _> = serde_json::from_str(&s);
                    assert!(ret.is_err(), ret);

                    let err = ret.unwrap_err();
                    assert!(
                        err.to_string()
                            .contains("number too large to fit in target type"),
                        err,
                    );
                }
            }
        };
    }

    test_json_uint!(uint32, Uint32, u32);
    test_json_uint!(uint64, Uint64, u64);
    test_json_uint!(uint128, Uint128, u128);
}
