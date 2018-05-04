#![allow(dead_code)]

extern crate openssl;

use self::openssl::pkey::PKey;
use self::openssl::rsa::Rsa as RsaGen;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Rsa {
    pub privkey_pkcs8: Vec<u8>,
    pub pubkey_der: Vec<u8>,
}

impl Default for Rsa {
    fn default() -> Self {
        let rsa = RsaGen::generate(2048).expect("Initialize Rsa");
        let pkey = PKey::from_rsa(rsa).expect("Initialize PKey");
        let privkey_pkcs8: Vec<u8> = pkey
            .private_key_to_der_pkcs8()
            .expect("Serialize privkey to pkcs8");
        let pubkey_der: Vec<u8> = pkey.public_key_to_der().expect("Serialize pubkey der");

        Rsa {
            privkey_pkcs8,
            pubkey_der,
        }
    }
}

impl Rsa {
    fn from_private_pem<T: AsRef<[u8]>>(privkey_pem: T) -> Self {
        let rsa = RsaGen::private_key_from_pem(privkey_pem.as_ref()).expect("Read privkey pem");
        let pkey = PKey::from_rsa(rsa).expect("Initialize PKey");
        let privkey_pkcs8: Vec<u8> = pkey
            .private_key_to_der_pkcs8()
            .expect("Serialize privkey to pkcs8");
        let pubkey_der: Vec<u8> = pkey.public_key_to_der().expect("Serialize pubkey der");

        Rsa {
            privkey_pkcs8,
            pubkey_der,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rsa_gen() {
        let _rsa = Rsa::default();
    }

    #[test]
    fn test_rsa_load() {
        let test_privkey = include_bytes!("test/private.pem");
        let test_privkey_pkcs8 = include_bytes!("test/private.pk8").to_vec();
        let test_pubkey = include_bytes!("test/public.der").to_vec();
        let rsa = Rsa::from_private_pem(&test_privkey[..]);

        assert_eq!(test_privkey_pkcs8, rsa.privkey_pkcs8);
        assert_eq!(test_pubkey, rsa.pubkey_der);
    }
}
