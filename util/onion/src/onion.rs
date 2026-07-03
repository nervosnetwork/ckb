use ed25519_dalek::hazmat::ExpandedSecretKey;
use ed25519_dalek::VerifyingKey;
use rand::RngCore;
use sha2::{Digest as _, Sha512};
use tiny_keccak::{Hasher, Sha3};

// Tor rend-spec-v3.txt, Section "ONIONADDRESS"
pub(crate) const ONION_CHECKSUM_PREFIX: &[u8] = b".onion checksum";
// Ed25519 keys are 32 bytes, then SHA-512 expanded to 64 bytes.
pub(crate) const SECRET_KEY_LENGTH: usize = 32;
pub(crate) const TORV3_SECRET_KEY_LENGTH: usize = ed25519_dalek::EXPANDED_SECRET_KEY_LENGTH;
pub(crate) const TORV3_PUBLIC_KEY_LENGTH: usize = ed25519_dalek::PUBLIC_KEY_LENGTH;

/// 32 public key bytes + 2 bytes of checksum = 34
/// (in onion address v3 there is one more byte - version eq to 3)
/// Checksum is embedded in order not to recompute it.
///
/// This variable denotes byte length of OnionAddressV3.
pub(crate) const TORV3_ONION_ADDRESS_LENGTH_BYTES: usize = 34;

/// Onion v3 service secret key (64-byte expanded Ed25519 key).
///
/// This matches the format expected by Tor's `ADD_ONION ED25519-V3` command.
#[derive(Clone)]
pub struct TorSecretKeyV3([u8; TORV3_SECRET_KEY_LENGTH]);

impl From<[u8; TORV3_SECRET_KEY_LENGTH]> for TorSecretKeyV3 {
    fn from(value: [u8; TORV3_SECRET_KEY_LENGTH]) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for TorSecretKeyV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TorSecretKeyV3")
            .field(&"<redacted>")
            .finish()
    }
}

impl TorSecretKeyV3 {
    /// Generate a new onion secret key.
    pub fn generate() -> Self {
        let mut secret_key = [0u8; SECRET_KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut secret_key);
        Self(expand_secret_key(&secret_key))
    }

    /// Return the raw 64-byte expanded secret key.
    pub fn as_bytes(&self) -> [u8; TORV3_SECRET_KEY_LENGTH] {
        self.0
    }

    /// Return the corresponding public key.
    pub fn public(&self) -> TorPublicKeyV3 {
        let expanded = ExpandedSecretKey::from_bytes(&self.0);
        TorPublicKeyV3(VerifyingKey::from(&expanded).to_bytes())
    }

    /// Encode the key as Tor expects in the `ADD_ONION` command.
    pub fn as_tor_proto_encoded(&self) -> String {
        data_encoding::BASE64.encode(&self.0)
    }
}

fn expand_secret_key(secret_key: &[u8; SECRET_KEY_LENGTH]) -> [u8; TORV3_SECRET_KEY_LENGTH] {
    let hash = Sha512::digest(secret_key);
    let mut expanded = [0u8; TORV3_SECRET_KEY_LENGTH];
    expanded.copy_from_slice(&hash);
    // Ed25519 clamping, applied to the first 32 bytes of the SHA-512 hash.
    expanded[0] &= 248;
    expanded[31] &= 127;
    expanded[31] |= 64;
    expanded
}

/// Onion v3 service public key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TorPublicKeyV3([u8; TORV3_PUBLIC_KEY_LENGTH]);

impl std::fmt::Debug for TorPublicKeyV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(
            f,
            "TorPublicKey({})",
            data_encoding::BASE32_NOPAD.encode(&self.0)
        )
    }
}

impl TorPublicKeyV3 {
    /// Derive the onion address from this public key.
    pub fn get_onion_address(&self) -> OnionAddressV3 {
        let mut buf = [0u8; 34];
        buf[..32].copy_from_slice(&self.0);

        let mut hasher = Sha3::v256();
        hasher.update(ONION_CHECKSUM_PREFIX);
        hasher.update(&self.0);
        hasher.update(b"\x03");
        let mut checksum = [0u8; 32];
        hasher.finalize(&mut checksum);

        buf[32] = checksum[0];
        buf[33] = checksum[1];

        OnionAddressV3(buf)
    }
}

/// Onion v3 address.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OnionAddressV3([u8; TORV3_ONION_ADDRESS_LENGTH_BYTES]);

impl std::fmt::Debug for OnionAddressV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(
            f,
            "OnionAddress({})",
            data_encoding::BASE32_NOPAD
                .encode(&self.get_raw_bytes())
                .to_ascii_lowercase()
        )
    }
}
impl std::fmt::Display for OnionAddressV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(
            f,
            "{}.onion",
            data_encoding::BASE32_NOPAD
                .encode(&self.get_raw_bytes())
                .to_ascii_lowercase()
        )
    }
}

impl OnionAddressV3 {
    /// Returns the 35-byte raw data that this onion address represents.
    ///
    /// The returned array consists of:
    /// - 32 bytes of the ed25519 public key,
    /// - 2 bytes of the truncated SHA3-256 checksum,
    /// - 1 version byte (`0x03`).
    #[inline]
    pub fn get_raw_bytes(&self) -> [u8; 35] {
        let mut buf = [0u8; 35];
        buf[..34].clone_from_slice(&self.0);
        buf[34] = 3;
        buf
    }

    /// Return the address without the `.onion` suffix.
    pub fn get_address_without_dot_onion(&self) -> String {
        data_encoding::BASE32_NOPAD
            .encode(&self.get_raw_bytes())
            .to_ascii_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_roundtrip() {
        let key = TorSecretKeyV3::generate();
        let public = key.public();
        let address = public.get_onion_address();
        let s = address.to_string();
        assert!(s.ends_with(".onion"));
        assert_eq!(s.len() - ".onion".len(), 56);

        let key2: TorSecretKeyV3 = key.as_bytes().into();
        assert_eq!(key2.public().0, public.0);
    }

    #[test]
    fn test_generated_key_clamping() {
        for _ in 0..10 {
            let key = TorSecretKeyV3::generate();
            let bytes = key.as_bytes();
            assert_eq!(bytes[0] & 0b0000_0111, 0, "byte 0 low 3 bits must be clear");
            assert_eq!(
                bytes[31] & 0b1100_0000,
                0b0100_0000,
                "byte 31 clamping wrong"
            );
        }
    }

    #[test]
    fn test_public_key_consistency() {
        let key = TorSecretKeyV3::generate();
        let pk1 = key.public();
        let pk2 = key.public();
        assert_eq!(pk1, pk2, "public key should be deterministic");
    }

    #[test]
    fn test_different_keys_different_public() {
        let k1 = TorSecretKeyV3::generate();
        let k2 = TorSecretKeyV3::generate();
        assert_ne!(k1.public(), k2.public(), "two random keys should differ");
    }

    #[test]
    fn test_address_consistency() {
        let key = TorSecretKeyV3::generate();
        let pk = key.public();
        let addr1 = pk.get_onion_address();
        let addr2 = pk.get_onion_address();
        assert_eq!(addr1, addr2, "address must be deterministic");
    }

    #[test]
    fn test_address_format() {
        let key = TorSecretKeyV3::generate();
        let addr_str = key.public().get_onion_address().to_string();
        assert!(addr_str.ends_with(".onion"));
        let host = &addr_str[..addr_str.len() - ".onion".len()];
        assert_eq!(host.len(), 56, "host part should be 56 chars");
        assert!(host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() && ('2'..='7').contains(&c)));
    }

    #[test]
    fn test_address_display_and_without_dot_onion() {
        let addr = TorSecretKeyV3::generate().public().get_onion_address();
        let full = addr.to_string();
        let without = addr.get_address_without_dot_onion();
        assert_eq!(full, format!("{}.onion", without));
    }

    #[test]
    fn test_base64_encoding_roundtrip() {
        let key = TorSecretKeyV3::generate();
        let encoded = key.as_tor_proto_encoded();
        let decoded = data_encoding::BASE64
            .decode(encoded.as_bytes())
            .expect("base64 decode should succeed");
        assert_eq!(decoded.len(), TORV3_SECRET_KEY_LENGTH);
        assert_eq!(decoded.as_slice(), &key.as_bytes());
    }

    #[test]
    fn test_onion_address_known_vector() {
        let seed = b"test onion key v3  1234";
        let hash = Sha512::digest(seed);
        let mut seed_bytes = [0u8; SECRET_KEY_LENGTH];
        seed_bytes.copy_from_slice(&hash[..SECRET_KEY_LENGTH]);

        let expanded = expand_secret_key(&seed_bytes);
        let secret = TorSecretKeyV3::from(expanded);

        let public = secret.public();
        let addr = public.get_onion_address();
        let addr_str = addr.to_string();

        assert_eq!(
            addr_str, "j3n6m6tyw5wrjcettpas6kvipurmappput67tljimrufpam7gh4tx4id.onion",
            "address should match independently computed one"
        );
    }

    #[test]
    fn test_checksum_embedded_correctly() {
        let pk = TorSecretKeyV3::generate().public();
        let addr = pk.get_onion_address();

        let embedded_checksum = [addr.0[32], addr.0[33]];

        let mut hasher = Sha3::v256();
        hasher.update(ONION_CHECKSUM_PREFIX);
        hasher.update(&pk.0);
        hasher.update(b"\x03");
        let mut computed = [0u8; 32];
        hasher.finalize(&mut computed);

        assert_eq!(
            embedded_checksum,
            [computed[0], computed[1]],
            "checksum mismatch"
        );
    }

    #[test]
    fn test_expand_preserves_hash_suffix() {
        let mut rng = rand::thread_rng();
        let mut seed = [0u8; SECRET_KEY_LENGTH];
        rng.fill_bytes(&mut seed);
        let full_hash = Sha512::digest(seed);
        let expanded = expand_secret_key(&seed);
        assert_eq!(&expanded[32..], &full_hash[32..]);
    }
}
