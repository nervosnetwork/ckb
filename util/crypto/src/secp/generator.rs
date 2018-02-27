use error::Error;
use rand::{self, ThreadRng};
use secp::SECP256K1;
use secp::privkey::Privkey;
use secp::pubkey::Pubkey;
use secp::secp256k1::key;

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
