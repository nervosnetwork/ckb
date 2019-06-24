use faster_hex::hex_decode;

use super::error::Error;

pub fn get_value<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Result<&'a serde_json::Value, Error> {
    value
        .get(field)
        .ok_or_else(|| format!("{} field not found", field))
        .map_err(Error::ParseJsonFailed)
}

pub fn get_u64(value: &serde_json::Value, field: &str) -> Result<u64, Error> {
    get_value(value, field)?
        .as_u64()
        .ok_or_else(|| format!("field {} is not integer", field))
        .map_err(Error::ParseJsonFailed)
}

pub fn get_str<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a str, Error> {
    get_value(value, field)?
        .as_str()
        .ok_or_else(|| format!("field {} is not string", field))
        .map_err(Error::ParseJsonFailed)
}

pub fn get_hex_bin(value: &serde_json::Value, field: &str) -> Result<Vec<u8>, Error> {
    get_str(value, field).and_then(|hex_str| {
        if hex_str.len() % 2 != 0 {
            return Err(Error::ParseJsonFailed(format!(
                "field {} odd hex string length: {}",
                field,
                hex_str.len()
            )));
        }
        let mut bin = vec![0u8; hex_str.len() / 2];
        hex_decode(hex_str.as_bytes(), &mut bin)
            .map_err(|err| format!("parse {} from hex error: {}", field, err))
            .map_err(Error::ParseJsonFailed)?;
        Ok(bin)
    })
}
