use super::PublicKey;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyGroup {
    pub data: HashMap<PublicKey, (Vec<u8>, Vec<u8>)>,
}

impl KeyGroup {
    pub fn insert(
        &mut self,
        signer_public_key: PublicKey,
        miner_public_key: Vec<u8>,
        miner_public_g: Vec<u8>,
    ) {
        self.data
            .insert(signer_public_key, (miner_public_key, miner_public_g));
    }

    pub fn get(&self, signer_public_key: &PublicKey) -> Option<(Vec<u8>, Vec<u8>)> {
        self.data.get(signer_public_key).cloned()
    }
}
