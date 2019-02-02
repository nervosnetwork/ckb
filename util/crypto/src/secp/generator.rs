use super::privkey::Privkey;
use super::pubkey::Pubkey;
use super::SECP256K1;
use rand;
use secp256k1::key::{PublicKey, SecretKey};

pub struct Generator;

impl Generator {
    pub fn new() -> Self {
        Generator {}
    }

    pub fn random_privkey(&self) -> Privkey {
        loop {
            if let Ok(sec) = SecretKey::from_slice(&rand::random::<
                [u8; secp256k1::constants::SECRET_KEY_SIZE],
            >()) {
                return sec.into();
            }
        }
    }

    pub fn random_keypair(self) -> (Privkey, Pubkey) {
        let privkey = self.random_privkey();
        let sec = SecretKey::from_slice(privkey.as_bytes()).unwrap();
        let publ = PublicKey::from_secret_key(&SECP256K1, &sec);

        (privkey, publ.into())
    }
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}
