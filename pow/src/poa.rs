use super::PowEngine;
use ckb_crypto::secp::{Message, Signature};
use ckb_hash::blake2b_256;
use ckb_jsonrpc_types::JsonBytes;
use ckb_types::{
    bytes::Bytes,
    core::HeaderContext,
    packed,
    prelude::*,
    utilities::{merkle_root, MerkleProof},
    H160, H256,
};
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct POAEngineConfig {
    pub pubkey_hash: JsonBytes,
}

pub struct POAEngine {
    config: POAEngineConfig,
}

impl POAEngine {
    pub fn new(config: POAEngineConfig) -> Self {
        POAEngine { config }
    }
}

impl PowEngine for POAEngine {
    fn verify(&self, header_ctx: &HeaderContext) -> bool {
        let cellbase = match header_ctx.cellbase() {
            Some(cb) => cb,
            None => {
                return false;
            }
        };

        let witness: Bytes = match cellbase.witnesses().get(0) {
            Some(w) => w.unpack(),
            None => {
                return false;
            }
        };
        let cellbase_witness = match packed::CellbaseExtWitness::from_slice(&witness) {
            Ok(w) => w,
            Err(_) => {
                return false;
            }
        };

        let poa_witness = match packed::POAWitness::from_slice(&Unpack::<Bytes>::unpack(
            &cellbase_witness.extension(),
        )) {
            Ok(w) => w,
            Err(_) => {
                return false;
            }
        };
        // verify merkle proof
        let proof_leaves = &[cellbase.calc_witness_hash()];
        let merkle_proof = match MerkleProof::build_proof(
            vec![0],
            proof_leaves,
            poa_witness.witnesses_root_proof().into_iter().collect(),
            poa_witness.transactions_count().unpack(),
        ) {
            Some(proof) => proof,
            None => {
                return false;
            }
        };
        let witness_root = match merkle_proof.root(proof_leaves) {
            Some(root) => root,
            None => {
                return false;
            }
        };
        let tx_root = merkle_root(&[poa_witness.raw_transactions_root(), witness_root]);
        if tx_root != header_ctx.header().transactions_root() {
            return false;
        }

        // remove signature from cellbase
        let cellbase_without_signature = {
            let poa_witness_without_signature = poa_witness
                .clone()
                .as_builder()
                .signature(packed::Byte65::new_unchecked(vec![0u8; 65].into()))
                .build();
            let cellbase_witness_without_signature = cellbase_witness
                .clone()
                .as_builder()
                .extension(poa_witness_without_signature.as_bytes().pack())
                .build();
            cellbase
                .clone()
                .as_advanced_builder()
                .set_witnesses(vec![cellbase_witness_without_signature.as_bytes().pack()])
                .build()
        };

        // calculate message
        // 1. use cellbase without signature to calculate a new witness_root
        // 2. calculate tx_root from new witness_root
        // 3. replace exists tx_root of header
        // 4. use header hash as message
        let witness_root = match merkle_proof.root(&[cellbase_without_signature.witness_hash()]) {
            Some(root) => root,
            None => {
                return false;
            }
        };
        let tx_root = merkle_root(&[poa_witness.raw_transactions_root(), witness_root.clone()]);
        let block_hash = header_ctx
            .header()
            .data()
            .as_advanced_builder()
            .transactions_root(tx_root.clone())
            .build()
            .hash();
        let message = match Message::from_slice(block_hash.as_slice()) {
            Ok(m) => m,
            Err(_) => {
                return false;
            }
        };

        // verify signature
        let signature = match Signature::from_slice(poa_witness.signature().as_slice()) {
            Ok(s) => s,
            Err(_) => {
                return false;
            }
        };
        let pubkey = match signature.recover(&message) {
            Ok(k) => k,
            Err(_) => {
                return false;
            }
        };
        let pubkey_hash = blake2b_160(pubkey.serialize());
        self.config.pubkey_hash.as_bytes() == pubkey_hash.as_bytes()
    }
}

pub fn blake2b_160<T: AsRef<[u8]>>(s: T) -> H160 {
    let result = blake2b_256(s);
    H160::from_slice(&result[..20]).expect("H160")
}
