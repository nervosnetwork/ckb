use super::{ProofPublicG, ProofPublickey};
use bigint::{H160, H256, U256};
use bls;
use difficulty::boundary_to_difficulty;
use global::TIME_STEP;
use hash::{Sha3, sha3_256};

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct Proof {
    pub sig: [u8; 21],
}

impl Proof {
    // generate proof
    pub fn new(private_key: &H160, time: u64, height: u64, challenge: &H256) -> Proof {
        let mut hash = [0u8; 32];
        let h1 = H256::from(time / TIME_STEP);
        let h2 = H256::from(height);
        let h3 = challenge;
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&h1);
        sha3.update(&h2);
        sha3.update(h3);
        sha3.finalize(&mut hash);
        Proof {
            sig: bls::sign(&hash, &private_key.0),
        }
    }

    pub fn from_slice(src: &[u8]) -> Proof {
        let mut sig = [0u8; 21];
        sig.clone_from_slice(src);
        Proof { sig }
    }

    /// verify the proof.
    pub fn verify(
        &self,
        time: u64,
        height: u64,
        challenge: &H256,
        pubkey: &ProofPublickey,
        g: &ProofPublicG,
    ) -> bool {
        let mut hash = [0u8; 32];
        let h1 = H256::from(time / TIME_STEP);
        let h2 = H256::from(height);
        let h3 = challenge;
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&h1);
        sha3.update(&h2);
        sha3.update(h3);
        sha3.finalize(&mut hash);
        bls::verify(&hash, &self.sig, &pubkey.0, &g.0)
    }

    pub fn hash(&self) -> H256 {
        sha3_256(&self.sig).into()
    }

    /// Get difficulty
    pub fn difficulty(&self) -> U256 {
        boundary_to_difficulty(&self.hash())
    }
}
