use super::error::Error;
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
        let mut random_slice: [u8; secp256k1::constants::SECRET_KEY_SIZE] = rand::random();
        loop {
            match SecretKey::from_slice(&random_slice) {
                Ok(sec) => return sec.into(),
                Err(_) => {
                    random_slice = rand::random();
                }
            }
        }
    }

    pub fn random_keypair(self) -> Result<(Privkey, Pubkey), Error> {
        let random_slice: [u8; secp256k1::constants::SECRET_KEY_SIZE] = rand::random();
        let sec = SecretKey::from_slice(&random_slice)?;
        let publ = PublicKey::from_secret_key(&SECP256K1, &sec);

        Ok((sec.into(), publ.into()))
    }
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}
