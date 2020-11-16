use super::privkey::Privkey;
use super::pubkey::Pubkey;
use super::SECP256K1;
use rand::{self, Rng, SeedableRng};
use secp256k1::{PublicKey, SecretKey};

/// A random secp keypair generator
pub struct Generator {
    rng: Box<dyn rand::RngCore>,
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}

impl Generator {
    /// Create a new Generator
    ///
    /// Default random number generator is `rand::rngs::ThreadRng`
    pub fn new() -> Self {
        let rng = rand::thread_rng();
        Generator { rng: Box::new(rng) }
    }

    /// Non crypto safe prng, should only used in tests
    pub fn non_crypto_safe_prng(seed: u64) -> Self {
        let rng = rand::rngs::SmallRng::seed_from_u64(seed);
        Generator { rng: Box::new(rng) }
    }

    /// Generate a SecretKey
    fn gen_secret_key(&mut self) -> SecretKey {
        let mut seed = vec![0; 32];
        loop {
            self.rng.fill(seed.as_mut_slice());
            if let Ok(key) = SecretKey::from_slice(&seed) {
                return key;
            }
        }
    }

    /// Generate a Privkey
    pub fn gen_privkey(&mut self) -> Privkey {
        self.gen_secret_key().into()
    }

    /// Generate a keypair
    pub fn gen_keypair(&mut self) -> (Privkey, Pubkey) {
        let secret_key = self.gen_secret_key();
        let pubkey = PublicKey::from_secret_key(&*SECP256K1, &secret_key);

        (secret_key.into(), pubkey.into())
    }

    /// Shortcuts construct temporary Generator, and generate a Privkey
    pub fn random_privkey() -> Privkey {
        Generator::new().gen_privkey()
    }

    /// Shortcuts construct temporary Generator, and generate a keypair
    pub fn random_keypair() -> (Privkey, Pubkey) {
        Generator::new().gen_keypair()
    }

    /// Shortcuts construct temporary Generator, and generate a SecretKey
    pub fn random_secret_key() -> SecretKey {
        Generator::new().gen_secret_key()
    }
}
