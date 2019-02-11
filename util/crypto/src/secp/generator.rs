use super::error::Error;
use super::privkey::Privkey;
use super::pubkey::Pubkey;
use super::SECP256K1;
use rand;
use secp256k1::key;

pub struct Generator;

impl Generator {
    pub fn new() -> Self {
        Generator {}
    }

    pub fn random_privkey(&self) -> Privkey {
        let mut rng = rand::thread_rng();
        key::SecretKey::new(&SECP256K1, &mut rng).into()
    }

    pub fn random_keypair(self) -> Result<(Privkey, Pubkey), Error> {
        let mut rng = rand::thread_rng();

        let (sec, publ) = SECP256K1.generate_keypair(&mut rng)?;

        Ok((sec.into(), publ.into()))
    }
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}
