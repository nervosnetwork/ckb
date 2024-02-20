use crate::{Byte32, Uint128, Uint32, Uint64};
use schemars::JsonSchema;

impl JsonSchema for Byte32 {
    fn schema_name() -> String {
        String::from("Byte32")
    }
    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        gen.subschema_for::<[u8; 32]>().into_object().into()
    }
}

impl JsonSchema for Uint32 {
    fn schema_name() -> String {
        String::from("Uint32")
    }
    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        gen.subschema_for::<u64>().into_object().into()
    }
}

impl JsonSchema for Uint64 {
    fn schema_name() -> String {
        String::from("Uint64")
    }
    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        gen.subschema_for::<u64>().into_object().into()
    }
}

impl JsonSchema for Uint128 {
    fn schema_name() -> String {
        String::from("Uint128")
    }
    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        gen.subschema_for::<u128>().into_object().into()
    }
}

pub fn u256_json_schema(
    _schemars: &mut schemars::gen::SchemaGenerator,
) -> schemars::schema::Schema {
    schemars::schema::SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::String.into()),
        format: Some("uint256".to_string()),
        ..Default::default()
    }
    .into()
}

pub fn rational_u256(_schemars: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    schemars::schema::SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::String.into()),
        format: Some("rational_u256".to_string()),
        ..Default::default()
    }
    .into()
}
