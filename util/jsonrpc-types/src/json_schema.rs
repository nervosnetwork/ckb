use crate::{Byte32, Uint32, Uint64, Uint128};
use schemars::JsonSchema;
use std::borrow::Cow;

macro_rules! impl_json_schema_for_type {
    ($type:ty, $inner_ty:ty, $name:expr) => {
        impl JsonSchema for $type {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed($name)
            }
            fn json_schema(gn: &mut schemars::SchemaGenerator) -> schemars::Schema {
                gn.subschema_for::<$inner_ty>()
            }
        }
    };
}

impl_json_schema_for_type!(Byte32, [u8; 32], "Byte32");
impl_json_schema_for_type!(Uint32, u32, "Uint32");
impl_json_schema_for_type!(Uint64, u64, "Uint64");
impl_json_schema_for_type!(Uint128, u128, "Uint128");

pub fn u256_json_schema(
    _schemars: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "format": "uint256"
    })
}

pub fn rational_u256(_schemars: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "format": "rational_u256"
    })
}
