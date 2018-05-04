use super::error::Error;
use super::privkey::Privkey;
use super::pubkey::Pubkey;
use super::secp256k1::key;
use super::SECP256K1;
use rand::{self, ThreadRng};

pub struct Generator {
    ///thread-local random number generator Rc<RefCell<_>>
    rng: ThreadRng,
}

impl Generator {
    pub fn new() -> Self {
        let rng = rand::thread_rng();
        Generator { rng }
    }

    pub fn random_privkey(&self) -> Privkey {
        let mut rng = self.rng.clone();
        key::SecretKey::new(&SECP256K1, &mut rng).into()
    }

    pub fn random_keypair(self) -> Result<(Privkey, Pubkey), Error> {
        let mut rng = self.rng.clone();

        let (sec, publ) = SECP256K1.generate_keypair(&mut rng)?;

        Ok((sec.into(), publ.into()))
    }
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}
