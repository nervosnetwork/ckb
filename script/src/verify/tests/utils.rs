use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_crypto::secp::{Generator, Privkey, Pubkey, Signature};
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainDB};
use ckb_types::{
    core::{
        hardfork::HardForkSwitch, Cycle, EpochNumber, EpochNumberWithFraction, HeaderView,
        TransactionInfo,
    },
    packed::{Byte32, TransactionInfoBuilder, TransactionKeyBuilder},
};
use faster_hex::hex_encode;
use std::{fs::File, path::Path};

use crate::verify::*;

pub(crate) const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;
pub(crate) const CYCLE_BOUND: Cycle = 200_000;

fn sha3_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
    use tiny_keccak::{Hasher, Sha3};
    let mut output = [0; 32];
    let mut sha3 = Sha3::v256();
    sha3.update(s.as_ref());
    sha3.finalize(&mut output);
    output
}

pub(crate) fn open_cell_always_success() -> File {
    File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/always_success")).unwrap()
}

pub(crate) fn open_cell_always_failure() -> File {
    File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/always_failure")).unwrap()
}

pub(crate) fn random_keypair() -> (Privkey, Pubkey) {
    Generator::random_keypair()
}

pub(crate) fn to_hex_pubkey(pubkey: &Pubkey) -> Vec<u8> {
    let pubkey = pubkey.serialize();
    let mut hex_pubkey = vec![0; pubkey.len() * 2];
    hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
    hex_pubkey
}

pub(crate) fn to_hex_signature(signature: &Signature) -> Vec<u8> {
    let signature_der = signature.serialize_der();
    let mut hex_signature = vec![0; signature_der.len() * 2];
    hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
    hex_signature
}

pub(crate) fn sign_args(args: &[u8], privkey: &Privkey) -> Signature {
    let hash = sha3_256(sha3_256(args));
    privkey.sign_recoverable(&hash.into()).unwrap()
}

pub(crate) fn default_transaction_info() -> TransactionInfo {
    TransactionInfoBuilder::default()
        .block_number(1u64.pack())
        .block_epoch(0u64.pack())
        .key(
            TransactionKeyBuilder::default()
                .block_hash(Byte32::zero())
                .index(1u32.pack())
                .build(),
        )
        .build()
        .unpack()
}

pub(crate) struct TransactionScriptsVerifierWithEnv {
    store: ChainDB,
    version_1_enabled_at: EpochNumber,
    consensus: Consensus,
}

impl TransactionScriptsVerifierWithEnv {
    pub(crate) fn new() -> Self {
        let store = ChainDB::new(RocksDB::open_tmp(COLUMNS), Default::default());
        let version_1_enabled_at = 10;
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_0032(version_1_enabled_at)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        Self {
            store,
            version_1_enabled_at,
            consensus,
        }
    }

    pub(crate) fn verify_without_limit(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
    ) -> Result<Cycle, Error> {
        self.verify(version, rtx, u64::MAX)
    }

    // If the max cycles is meaningless, please use `verify_without_limit`,
    // so reviewers or developers can understand the intentions easier.
    pub(crate) fn verify(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
        max_cycles: Cycle,
    ) -> Result<Cycle, Error> {
        self.verify_map(version, rtx, |verifier| verifier.verify(max_cycles))
    }

    pub(crate) fn verify_map<R, F>(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
        mut verify_func: F,
    ) -> R
    where
        F: FnMut(TransactionScriptsVerifier<'_, DataLoaderWrapper<'_, ChainDB>>) -> R,
    {
        let data_loader = DataLoaderWrapper::new(&self.store);

        let epoch = match version {
            ScriptVersion::V0 => EpochNumberWithFraction::new(0, 0, 1),
            ScriptVersion::V1 => EpochNumberWithFraction::new(self.version_1_enabled_at, 0, 1),
        };
        let header = HeaderView::new_advanced_builder()
            .epoch(epoch.pack())
            .build();
        let tx_env = TxVerifyEnv::new_commit(&header);

        let verifier = TransactionScriptsVerifier::new(rtx, &self.consensus, &data_loader, &tx_env);
        verify_func(verifier)
    }
}
