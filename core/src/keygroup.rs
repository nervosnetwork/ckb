use super::{ProofPublicG, ProofPublickey, PublicKey};
use bigint::H328;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyGroup {
    pub data: HashMap<PublicKey, (H328, H328)>,
}

impl KeyGroup {
    pub fn with_capacity(capacity: usize) -> KeyGroup {
        KeyGroup {
            data: HashMap::with_capacity(capacity),
        }
    }

    pub fn insert(
        &mut self,
        signer_public_key: PublicKey,
        miner_public_key: ProofPublickey,
        miner_public_g: ProofPublicG,
    ) {
        self.data
            .insert(signer_public_key, (miner_public_key, miner_public_g));
    }

    pub fn get(&self, signer_public_key: &PublicKey) -> Option<(H328, H328)> {
        self.data.get(signer_public_key).cloned()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
