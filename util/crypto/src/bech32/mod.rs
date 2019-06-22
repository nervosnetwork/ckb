//! The Bech32 encoding was originally formulated in [BIP-0173](https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki)
//!
//! # Examples
//!
//! ```rust
//! use ckb_crypto::bech32::Bech32;
//!
//! let b = Bech32 {
//!     hrp: "bech32".to_string(),
//!     data: vec![0x00, 0x01, 0x02]
//! };
//! let encoded = b.encode().unwrap();
//! assert_eq!(encoded, "bech321qpz4nc4pe".to_string());
//!
//! let c = Bech32::decode(&encoded);
//! assert_eq!(b, c.unwrap());
//! ```

mod error;

pub use self::error::Error;
use crunchy::unroll;

// Generator coefficients
const GEN: [u32; 5] = [
    0x3b6a_57b2,
    0x2650_8e6d,
    0x1ea1_19fa,
    0x3d42_33dd,
    0x2a14_62b3,
];

// The separator, which is always "1
const SEP: char = '1';

/// Encoding character set.
const CHARSET: [char; 32] = [
    'q', 'p', 'z', 'r', 'y', '9', 'x', '8', 'g', 'f', '2', 't', 'v', 'd', 'w', '0', 's', '3', 'j',
    'n', '5', '4', 'k', 'h', 'c', 'e', '6', 'm', 'u', 'a', '7', 'l',
];

// Maps ASCII byte -> CHARSET index on [0,31]
const CHARSET_IDX: [i8; 128] = [
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    15, -1, 10, 17, 21, 20, 26, 30, 7, 5, -1, -1, -1, -1, -1, -1, -1, 29, -1, 24, 13, 25, 9, 8, 23,
    -1, 18, 22, 31, 27, 19, -1, 1, 0, 3, 16, 11, 28, 12, 14, 6, 4, 2, -1, -1, -1, -1, -1, -1, 29,
    -1, 24, 13, 25, 9, 8, 23, -1, 18, 22, 31, 27, 19, -1, 1, 0, 3, 16, 11, 28, 12, 14, 6, 4, 2, -1,
    -1, -1, -1, -1,
];

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct Bech32 {
    // The human-readable part, which is intended to convey the type of data, or anything else that is relevant to the reader.
    // This part MUST contain 1 to 83 US-ASCII characters, with each character having a value in the range [33-126].
    // HRP validity may be further restricted by specific applications
    pub hrp: String,
    // The data part, which is at least 6 characters long and only consists of alphanumeric characters excluding "1", "b", "i", and "o"
    pub data: Vec<u8>,
}

impl Bech32 {
    pub fn new(hrp: String, data: Vec<u8>) -> Self {
        Bech32 { hrp, data }
    }

    pub fn encode(&self) -> Result<String, Error> {
        if self.hrp.is_empty() {
            return Err(Error::InvalidLength);
        }
        let hrp_bytes: &[u8] = self.hrp.as_bytes();
        let mut combined: Vec<u8> = self.data.clone();
        combined.extend_from_slice(&create_checksum(hrp_bytes, &self.data));
        let mut encoded: String = format!("{}{}", self.hrp, SEP);
        for p in combined {
            if p >= 32 {
                return Err(Error::InvalidData(p));
            }
            encoded.push(CHARSET[p as usize]);
        }
        Ok(encoded)
    }

    pub fn decode<S: AsRef<str>>(s: S) -> Result<Bech32, Error> {
        // Ensure overall length is within bounds
        let s = s.as_ref();
        let len: usize = s.len();
        if len < 8 || len > 90 {
            return Err(Error::InvalidLength);
        }

        // Check for missing separator
        if s.find(SEP).is_none() {
            return Err(Error::MissingSeparator);
        }

        let parts: Vec<&str> = s.rsplitn(2, SEP).collect();
        let raw_hrp = parts[1];
        let raw_data = parts[0];
        if raw_hrp.is_empty() || raw_data.len() < 6 {
            return Err(Error::InvalidLength);
        }

        let mut has_lower: bool = false;
        let mut has_upper: bool = false;
        let mut hrp_bytes: Vec<u8> = Vec::with_capacity(raw_hrp.len());
        for b in raw_hrp.bytes() {
            // Valid subset of ASCII
            if b < 33 || b > 126 {
                Err(Error::InvalidChar(b))?;
            }
            // Lowercase
            if b.is_ascii_lowercase() {
                has_lower = true;
            }
            // Uppercase
            let c = if b.is_ascii_uppercase() {
                has_upper = true;
                // Convert to lowercase
                b.to_ascii_lowercase()
            } else {
                b
            };

            hrp_bytes.push(c);
        }

        // Check data payload
        let mut data_bytes: Vec<u8> = Vec::with_capacity(raw_data.len());
        for b in raw_data.bytes() {
            // Aphanumeric only
            if !((b >= b'0' && b <= b'9') || (b >= b'A' && b <= b'Z') || (b >= b'a' && b <= b'z')) {
                return Err(Error::InvalidChar(b));
            }
            // Excludes these characters: [1,b,i,o]
            if b == b'1' || b == b'b' || b == b'i' || b == b'o' {
                return Err(Error::InvalidChar(b));
            }
            // Lowercase
            if b.is_ascii_lowercase() {
                has_lower = true;
            }

            // Uppercase
            let c = if b.is_ascii_uppercase() {
                has_upper = true;
                // Convert to lowercase
                b.to_ascii_lowercase()
            } else {
                b
            };

            data_bytes.push(CHARSET_IDX[c as usize] as u8);
        }

        // Ensure no mixed case
        if has_lower && has_upper {
            return Err(Error::MixedCase);
        }

        // Ensure checksum
        if !verify_checksum(&hrp_bytes, &data_bytes) {
            return Err(Error::InvalidChecksum);
        }

        // Remove checksum from data payload
        let dbl: usize = data_bytes.len();
        data_bytes.truncate(dbl - 6);

        Ok(Bech32 {
            hrp: String::from_utf8(hrp_bytes)?,
            data: data_bytes,
        })
    }
}

fn polymod(values: &[u8]) -> u32 {
    let mut chk: u32 = 1;
    let mut b: u8;
    for v in values {
        b = (chk >> 25) as u8;
        chk = (chk & 0x1ff_ffff) << 5 ^ (u32::from(*v));
        // constant-fold
        unroll! {
            for i in 0..5 {
                if (b >> i) & 1 == 1 {
                    chk ^= GEN[i]
                }
            }
        }
    }
    chk
}

fn hrp_expand(hrp: &[u8]) -> Vec<u8> {
    let mut v = hrp
        .iter()
        .fold(Vec::with_capacity(hrp.len() * 2 + 1), |mut acc, x| {
            acc.push(*x >> 5);
            acc
        });
    v.push(0);
    hrp.iter().fold(v, |mut acc, x| {
        acc.push(*x & 0x1f);
        acc
    })
}

fn verify_checksum(hrp: &[u8], data: &[u8]) -> bool {
    let mut expand = hrp_expand(hrp);
    expand.extend_from_slice(data);
    polymod(&expand[..]) == 1u32
}

fn create_checksum(hrp: &[u8], data: &[u8]) -> Vec<u8> {
    let mut values: Vec<u8> = hrp_expand(hrp);
    values.extend_from_slice(data);
    values.extend_from_slice(&[0u8; 6]);
    let plm: u32 = polymod(&values[..]) ^ 1;

    (0..6).fold(Vec::with_capacity(6), |mut acc, x| {
        let i = ((plm >> (5 * (5 - x))) & 0x1f) as u8;
        acc.push(i);
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::{Bech32, Error};

    #[test]
    fn valid_checksum() {
        let strings: Vec<&str> = vec!(
            "A12UEL5L",
            "a12uel5l",
            "an83characterlonghumanreadablepartthatcontainsthenumber1andtheexcludedcharactersbio1tt5tgs",
            "abcdef1qpzry9x8gf2tvdw0s3jn54khce6mua7lmqqqxw",
            "11qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqc8247j",
            "split1checkupstagehandshakeupstreamerranterredcaperred2y9e3w",
            "?1ezyfcl",
        );
        for s in strings {
            let decoded = Bech32::decode(s);
            assert!(decoded.is_ok());

            let encoded = decoded.unwrap().encode();
            assert!(encoded.is_ok());
            assert_eq!(s.to_lowercase(), encoded.unwrap().to_lowercase());
        }
    }

    #[test]
    fn invalid() {
        let pairs: Vec<(&str, Error)> = vec!(
            (" 1nwldj5",
                Error::InvalidChar(b' ')),
            ("\x7f1axkwrx",
                Error::InvalidChar(0x7f)),
            ("an84characterslonghumanreadablepartthatcontainsthenumber1andtheexcludedcharactersbio1569pvx",
                Error::InvalidLength),
            ("pzry9x0s0muk",
                Error::MissingSeparator),
            ("1pzry9x0s0muk",
                Error::InvalidLength),
            ("x1b4n0q5v",
                Error::InvalidChar(b'b')),
            ("li1dgmt3",
                Error::InvalidLength),
            ("de1lg7wt\u{ff}",
                Error::InvalidChar(0xc3)), // ASCII 0xff -> \uC3BF in UTF-8
        );
        for p in pairs {
            let (s, expected_error) = p;
            let dec_result = Bech32::decode(s);
            println!("{:?}", s);
            if dec_result.is_ok() {
                println!("{:?}", dec_result.unwrap());
                panic!("Should be invalid: {:?}", s);
            }
            assert_eq!(dec_result.unwrap_err(), expected_error);
        }
    }
}
