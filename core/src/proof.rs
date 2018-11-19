use super::{ProofPublicG, ProofPublickey};
use bigint::{H256, U256};
use bls;
use difficulty::boundary_to_difficulty;
use global::TIME_STEP;
use hash::{Sha3, sha3_256};

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct Proof {
    pub sig: Vec<u8>,
}

impl Proof {
    // generate proof
    pub fn new(private_key: &[u8], time: u64, height: u64, challenge: &H256) -> Proof {
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
            sig: bls::sign(hash.to_vec(), private_key.to_vec()),
        }
    }

    /// verify the proof.
    pub fn verify(
        &self,
        time: u64,
        height: u64,
        challenge: H256,
        pubkey: ProofPublickey,
        g: ProofPublicG,
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
        bls::verify(hash.to_vec(), self.sig.clone(), pubkey.to_vec(), g.to_vec())
    }

    pub fn hash(&self) -> H256 {
        sha3_256(self.sig.as_slice()).into()
    }

    /// Get difficulty
    pub fn difficulty(&self) -> U256 {
        boundary_to_difficulty(&self.hash())
    }
}
