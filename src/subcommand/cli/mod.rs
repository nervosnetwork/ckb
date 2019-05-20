mod blake;
mod hashes;
mod secp256k1_lock;

pub use blake::{blake160, blake256};
pub use hashes::hashes;
pub use secp256k1_lock::secp256k1_lock;

use ckb_app_config::ExitCode;
use faster_hex::hex_decode;

fn canonicalize_data(data: &str) -> &str {
    let data = data.trim();
    if data.len() >= 2 && &data[..2] == "0x" {
        &data[2..]
    } else {
        data
    }
}

pub fn parse_hex_data(data: &str) -> Result<Vec<u8>, ExitCode> {
    let data = canonicalize_data(data);
    if data.len() % 2 != 0 {
        eprintln!("Malformed hex string: {}, error: length is odd", data);
        return Err(ExitCode::Cli);
    }

    let mut decoded = vec![];
    decoded.resize(data.len() / 2, 0);
    hex_decode(data.as_bytes(), decoded.as_mut_slice()).map_err(|err| {
        eprintln!("Malformed hex string: {}, error: {}", data, err);
        ExitCode::Cli
    })?;

    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_data() {
        assert!(parse_hex_data("0x0").is_err());
        assert!(parse_hex_data("0x0g").is_err());
        assert_eq!(parse_hex_data("01"), Ok(vec![1]));
        assert_eq!(parse_hex_data("0x01"), Ok(vec![1]));
    }
}
