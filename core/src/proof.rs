use bigint::{H256, U256};
use bls;
use hash::{Sha3, sha3_256};

const TIME_STEP: u64 = 1;

#[derive(Debug)]
pub struct Proof {
    sig: Vec<u8>,
}

impl Proof {
    // generate proof
    pub fn new(private_key: Vec<u8>, time: u64, height: u64, challenge: H256) -> Proof {
        let mut hash = [0u8; 32];
        let h1 = H256::from(time / TIME_STEP).to_vec();
        let h2 = H256::from(height).to_vec();
        let h3 = challenge.to_vec();
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&h1);
        sha3.update(&h2);
        sha3.update(&h3);
        sha3.finalize(&mut hash);
        Proof {
            sig: bls::sign(hash.to_vec(), private_key),
        }
    }

    /// verify the proof.
    pub fn verify(
        &self,
        time: u64,
        height: u64,
        challenge: H256,
        pubkey: Vec<u8>,
        g: Vec<u8>,
    ) -> bool {
        let mut hash = [0u8; 32];
        let h1 = H256::from(time / TIME_STEP).to_vec();
        let h2 = H256::from(height).to_vec();
        let h3 = challenge.to_vec();
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&h1);
        sha3.update(&h2);
        sha3.update(&h3);
        sha3.finalize(&mut hash);
        bls::verify(hash.to_vec(), self.sig.clone(), pubkey, g)
    }

    /// Get difficulty
    pub fn difficulty(&self) -> U256 {
        U256::from(sha3_256(self.sig.as_slice()))
    }
}
