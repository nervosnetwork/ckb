use base64::Engine;
use curve25519_dalek::{constants::ED25519_BASEPOINT_TABLE, scalar::Scalar};
use rand::RngCore;
use sha2::{Digest as Sha2Digest, Sha512};
use sha3::Sha3_256;

const TORV3_SECRET_KEY_LENGTH: usize = 64;
const TORV3_PUBLIC_KEY_LENGTH: usize = 32;
const TORV3_VERSION: u8 = 3;
const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

#[derive(Clone)]
pub struct TorSecretKeyV3([u8; TORV3_SECRET_KEY_LENGTH]);

impl TorSecretKeyV3 {
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        Self::from_seed(seed)
    }

    fn from_seed(seed: [u8; 32]) -> Self {
        let digest = Sha512::digest(seed);
        let mut bytes = [0u8; TORV3_SECRET_KEY_LENGTH];
        bytes.copy_from_slice(&digest);
        bytes[0] &= 248;
        bytes[31] &= 63;
        bytes[31] |= 64;
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; TORV3_SECRET_KEY_LENGTH]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> [u8; TORV3_SECRET_KEY_LENGTH] {
        self.0
    }

    pub fn to_tor_key_blob(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.0)
    }

    pub fn public_key_bytes(&self) -> [u8; TORV3_PUBLIC_KEY_LENGTH] {
        let mut scalar_bytes = [0u8; 32];
        scalar_bytes.copy_from_slice(&self.0[..32]);
        let scalar = Scalar::from_bytes_mod_order(scalar_bytes);
        let public = (ED25519_BASEPOINT_TABLE * &scalar).compress();
        public.to_bytes()
    }

    pub fn onion_address_without_dot_onion(&self) -> String {
        let public_key = self.public_key_bytes();
        let mut digest = Sha3_256::new();
        digest.update(b".onion checksum");
        digest.update(public_key);
        digest.update([TORV3_VERSION]);
        let checksum = digest.finalize();

        let mut onion_bytes = [0u8; 35];
        onion_bytes[..32].copy_from_slice(&public_key);
        onion_bytes[32] = checksum[0];
        onion_bytes[33] = checksum[1];
        onion_bytes[34] = TORV3_VERSION;
        base32_no_padding_lowercase(&onion_bytes)
    }
}

impl std::fmt::Display for TorSecretKeyV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TorSecretKey(****)")
    }
}

impl std::fmt::Debug for TorSecretKeyV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TorSecretKey(****)")
    }
}

fn base32_no_padding_lowercase(bytes: &[u8]) -> String {
    let mut output = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buffer = 0u16;
    let mut bits = 0u8;

    for &byte in bytes {
        buffer = (buffer << 8) | u16::from(byte);
        bits += 8;
        while bits >= 5 {
            let index = ((buffer >> (bits - 5)) & 0x1f) as usize;
            output.push(BASE32_ALPHABET[index] as char);
            bits -= 5;
        }
    }

    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        output.push(BASE32_ALPHABET[index] as char);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::TorSecretKeyV3;

    #[test]
    fn derives_ed25519_public_key_from_seed() {
        let seed = hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .unwrap()
            .try_into()
            .unwrap();
        let key = TorSecretKeyV3::from_seed(seed);
        assert_eq!(
            hex::encode(key.public_key_bytes()),
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
    }

    #[test]
    fn derives_onion_address_from_expanded_key() {
        let mut key = [0u8; 64];
        key[0] = 80;
        key[1] = 47;
        key[2] = 7;
        key[31] = 109;
        key[63] = 120;
        let key = TorSecretKeyV3::from_bytes(key);
        assert_eq!(key.onion_address_without_dot_onion().len(), 56);
    }
}
