use super::privkey::Privkey;
use super::pubkey::Pubkey;
use super::SECP256K1;
use rand::{self, Rng, SeedableRng};
use secp256k1::{PublicKey, SecretKey};

/// TODO(doc): @zhangsoledad
pub struct Generator {
    rng: Box<dyn rand::RngCore>,
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}

impl Generator {
    /// TODO(doc): @zhangsoledad
    pub fn new() -> Self {
        let rng = rand::thread_rng();
        Generator { rng: Box::new(rng) }
    }

    /// Non crypto safe prng, should only used in tests
    pub fn non_crypto_safe_prng(seed: u64) -> Self {
        let rng = rand::rngs::SmallRng::seed_from_u64(seed);
        Generator { rng: Box::new(rng) }
    }

    fn gen_secret_key(&mut self) -> SecretKey {
        let mut seed = vec![0; 32];
        loop {
            self.rng.fill(seed.as_mut_slice());
            if let Ok(key) = SecretKey::from_slice(&seed) {
                return key;
            }
        }
    }

    /// TODO(doc): @zhangsoledad
    pub fn gen_privkey(&mut self) -> Privkey {
        self.gen_secret_key().into()
    }

    /// TODO(doc): @zhangsoledad
    pub fn gen_keypair(&mut self) -> (Privkey, Pubkey) {
        let secret_key = self.gen_secret_key();
        let pubkey = PublicKey::from_secret_key(&*SECP256K1, &secret_key);

        (secret_key.into(), pubkey.into())
    }

    /// TODO(doc): @zhangsoledad
    pub fn random_privkey() -> Privkey {
        Generator::new().gen_privkey()
    }

    /// TODO(doc): @zhangsoledad
    pub fn random_keypair() -> (Privkey, Pubkey) {
        Generator::new().gen_keypair()
    }

    /// TODO(doc): @zhangsoledad
    pub fn random_secret_key() -> SecretKey {
        Generator::new().gen_secret_key()
    }
}
