use crate::{Byte32, Uint128, Uint32, Uint64};
use schemars::JsonSchema;

macro_rules! impl_json_schema_for_type {
    ($type:ty, $inner_ty:ty, $name:expr) => {
        impl JsonSchema for $type {
            fn schema_name() -> String {
                String::from($name)
            }
            fn json_schema(gn: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
                gn.subschema_for::<$inner_ty>().into_object().into()
            }
        }
    };
}

impl_json_schema_for_type!(Byte32, [u8; 32], "Byte32");
impl_json_schema_for_type!(Uint32, u32, "Uint32");
impl_json_schema_for_type!(Uint64, u64, "Uint64");
impl_json_schema_for_type!(Uint128, u128, "Uint128");

pub fn u256_json_schema(
    _schemars: &mut schemars::r#gen::SchemaGenerator,
) -> schemars::schema::Schema {
    schemars::schema::SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::String.into()),
        format: Some("uint256".to_string()),
        ..Default::default()
    }
    .into()
}

pub fn rational_u256(_schemars: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    schemars::schema::SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::String.into()),
        format: Some("rational_u256".to_string()),
        ..Default::default()
    }
    .into()
}
