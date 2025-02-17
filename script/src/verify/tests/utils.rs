use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_chain_spec::consensus::TWO_IN_TWO_OUT_BYTES;
use ckb_crypto::secp::{Generator, Privkey, Pubkey, Signature};
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_store::{
    data_loader_wrapper::{AsDataLoader, DataLoaderWrapper},
    ChainDB,
};
use ckb_test_chain_utils::{
    ckb_testnet_consensus, secp256k1_blake160_sighash_cell, secp256k1_data_cell,
    type_lock_script_code_hash,
};
use ckb_types::{
    core::{
        capacity_bytes,
        cell::{CellMeta, CellMetaBuilder},
        hardfork::{HardForks, CKB2021, CKB2023},
        Capacity, Cycle, DepType, EpochNumber, EpochNumberWithFraction, HeaderView, ScriptHashType,
        TransactionBuilder, TransactionInfo,
    },
    h256,
    packed::{
        Byte32, CellDep, CellInput, CellOutput, OutPoint, Script, TransactionInfoBuilder,
        TransactionKeyBuilder, WitnessArgs,
    },
    H256,
};
use faster_hex::hex_encode;
use std::sync::Arc;
use std::{fs::File, path::Path};
use tempfile::TempDir;

use crate::verify::*;

pub(crate) const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;
pub(crate) const CYCLE_BOUND: Cycle = 250_000;
pub(crate) const V2_CYCLE_BOUND: Cycle = 300_000;

fn sha3_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
    use tiny_keccak::{Hasher, Sha3};
    let mut output = [0; 32];
    let mut sha3 = Sha3::v256();
    sha3.update(s.as_ref());
    sha3.finalize(&mut output);
    output
}

pub(crate) fn open_cell_always_success() -> File {
    open_cell_file("testdata/always_success")
}

pub(crate) fn open_cell_always_failure() -> File {
    open_cell_file("testdata/always_failure")
}

pub(crate) fn open_cell_file(path_str: &str) -> File {
    File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join(path_str)).unwrap()
}

pub(crate) fn load_cell_from_path(path_str: &str) -> (CellMeta, Byte32) {
    let cell_data = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(path_str)).unwrap();
    load_cell_from_slice(&cell_data)
}

pub(crate) fn load_cell_from_slice(slice: &[u8]) -> (CellMeta, Byte32) {
    let cell_data = Bytes::copy_from_slice(slice);
    let cell_output = CellOutput::new_builder()
        .capacity(Capacity::bytes(cell_data.len()).unwrap().pack())
        .build();
    let cell_meta = CellMetaBuilder::from_cell_output(cell_output, cell_data)
        .transaction_info(default_transaction_info())
        .build();
    let data_hash = cell_meta.mem_cell_data_hash.as_ref().unwrap().to_owned();
    (cell_meta, data_hash)
}

pub(crate) fn create_dummy_cell(output: CellOutput) -> CellMeta {
    CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build()
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
    // The fields of a struct are dropped in declaration order.
    // So, put `ChainDB` (`RocksDB`) before `TempDir`.
    //
    // Ref: https://doc.rust-lang.org/reference/destructors.html
    store: Arc<ChainDB>,
    consensus: Arc<Consensus>,
    version_1_enabled_at: EpochNumber,
    version_2_enabled_at: EpochNumber,
    _tmp_dir: TempDir,
}

impl TransactionScriptsVerifierWithEnv {
    pub(crate) fn new() -> Self {
        let tmp_dir = TempDir::new().unwrap();
        let db = RocksDB::open_in(&tmp_dir, COLUMNS);
        let store = Arc::new(ChainDB::new(db, Default::default()));
        let version_1_enabled_at = 5;
        let version_2_enabled_at = 10;

        let hardfork_switch = HardForks {
            ckb2021: CKB2021::new_mirana()
                .as_builder()
                .rfc_0032(version_1_enabled_at)
                .build()
                .unwrap(),
            ckb2023: CKB2023::new_mirana()
                .as_builder()
                .rfc_0049(version_2_enabled_at)
                .build()
                .unwrap(),
        };
        let consensus = Arc::new(
            ConsensusBuilder::default()
                .hardfork_switch(hardfork_switch)
                .build(),
        );
        Self {
            store,
            version_1_enabled_at,
            version_2_enabled_at,
            consensus,
            _tmp_dir: tmp_dir,
        }
    }

    pub(crate) fn verify_without_limit(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
    ) -> Result<Cycle, Error> {
        self.verify(version, rtx, u64::MAX)
    }

    pub(crate) async fn verify_without_limit_async(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
    ) -> Result<Cycle, Error> {
        let data_loader = self.store.as_data_loader();
        let epoch = match version {
            ScriptVersion::V0 => EpochNumberWithFraction::new(0, 0, 1),
            ScriptVersion::V1 => EpochNumberWithFraction::new(self.version_1_enabled_at, 0, 1),
            ScriptVersion::V2 => EpochNumberWithFraction::new(self.version_2_enabled_at, 0, 1),
        };
        let header = HeaderView::new_advanced_builder()
            .epoch(epoch.pack())
            .build();
        let tx_env = Arc::new(TxVerifyEnv::new_commit(&header));
        let verifier = TransactionScriptsVerifier::new(
            Arc::new(rtx.clone()),
            data_loader,
            Arc::clone(&self.consensus),
            tx_env,
        );

        let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);
        verifier
            .resumable_verify_with_signal(u64::MAX, &mut command_rx)
            .await
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

    pub(crate) fn verify_without_pause(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
        max_cycles: Cycle,
    ) -> Result<Cycle, Error> {
        self.verify_map(version, rtx, |verifier| {
            verifier.set_skip_pause(true);
            verifier.verify(max_cycles)
        })
    }

    pub(crate) fn verify_until_completed(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
    ) -> Result<(Cycle, usize), Error> {
        let max_cycles = Cycle::MAX;
        self.verify_map(version, rtx, |verifier| {
            let cycles;
            let mut times = 0usize;
            times += 1;

            let mut init_snap = match verifier.resumable_verify(max_cycles).unwrap() {
                VerifyResult::Suspended(state) => Some(state),
                VerifyResult::Completed(cycle) => {
                    cycles = cycle;
                    return Ok((cycles, times));
                }
            };

            loop {
                times += 1;
                let snap = init_snap.take().unwrap();
                match verifier.resume_from_state(&snap, max_cycles) {
                    Ok(VerifyResult::Suspended(state)) => {
                        init_snap = Some(state);
                    }
                    Ok(VerifyResult::Completed(cycle)) => {
                        cycles = cycle;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }

            Ok((cycles, times))
        })
    }

    pub(crate) async fn verify_complete_async(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
        command_rx: &mut tokio::sync::watch::Receiver<ChunkCommand>,
        skip_debug_pause: bool,
    ) -> Result<Cycle, Error> {
        let data_loader = self.store.as_data_loader();
        let epoch = match version {
            ScriptVersion::V0 => EpochNumberWithFraction::new(0, 0, 1),
            ScriptVersion::V1 => EpochNumberWithFraction::new(self.version_1_enabled_at, 0, 1),
            ScriptVersion::V2 => EpochNumberWithFraction::new(self.version_2_enabled_at, 0, 1),
        };
        let header = HeaderView::new_advanced_builder()
            .epoch(epoch.pack())
            .build();
        let tx_env = Arc::new(TxVerifyEnv::new_commit(&header));
        let verifier = TransactionScriptsVerifier::new(
            Arc::new(rtx.clone()),
            data_loader,
            Arc::clone(&self.consensus),
            tx_env,
        );

        if skip_debug_pause {
            verifier.set_skip_pause(true);
        }
        verifier
            .resumable_verify_with_signal(Cycle::MAX, command_rx)
            .await
    }

    pub(crate) fn verify_map<R, F>(
        &self,
        version: ScriptVersion,
        rtx: &ResolvedTransaction,
        mut verify_func: F,
    ) -> R
    where
        F: FnMut(TransactionScriptsVerifier<DataLoaderWrapper<ChainDB>>) -> R,
    {
        let data_loader = self.store.as_data_loader();
        let epoch = match version {
            ScriptVersion::V0 => EpochNumberWithFraction::new(0, 0, 1),
            ScriptVersion::V1 => EpochNumberWithFraction::new(self.version_1_enabled_at, 0, 1),
            ScriptVersion::V2 => EpochNumberWithFraction::new(self.version_2_enabled_at, 0, 1),
        };
        let header = HeaderView::new_advanced_builder()
            .epoch(epoch.pack())
            .build();
        let tx_env = Arc::new(TxVerifyEnv::new_commit(&header));
        let mut verifier = TransactionScriptsVerifier::new(
            Arc::new(rtx.clone()),
            data_loader,
            Arc::clone(&self.consensus),
            tx_env,
        );
        verifier.set_debug_printer(Box::new(move |_hash: &Byte32, message: &str| {
            print!("{}", message);
            if !message.ends_with('\n') {
                println!();
            }
        }));
        verify_func(verifier)
    }
}

pub(super) fn random_2_in_2_out_rtx() -> ResolvedTransaction {
    let consensus = ckb_testnet_consensus();
    let dep_group_tx_hash = consensus.genesis_block().transactions()[1].hash();
    let secp_out_point = OutPoint::new(dep_group_tx_hash, 0);

    let cell_dep = CellDep::new_builder()
        .out_point(secp_out_point)
        .dep_type(DepType::DepGroup.into())
        .build();

    let input1 = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 0), 0);
    let input2 = CellInput::new(OutPoint::new(h256!("0x1111").pack(), 0), 0);

    let mut generator = Generator::non_crypto_safe_prng(42);
    let privkey = generator.gen_privkey();
    let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
    let lock_arg = Bytes::from((blake2b_256(pubkey_data)[0..20]).to_owned());
    let privkey2 = generator.gen_privkey();
    let pubkey_data2 = privkey2.pubkey().expect("Get pubkey failed").serialize();
    let lock_arg2 = Bytes::from((blake2b_256(pubkey_data2)[0..20]).to_owned());

    let lock = Script::new_builder()
        .args(lock_arg.pack())
        .code_hash(type_lock_script_code_hash().pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let lock2 = Script::new_builder()
        .args(lock_arg2.pack())
        .code_hash(type_lock_script_code_hash().pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let output1 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock.clone())
        .build();
    let output2 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock2.clone())
        .build();
    let tx = TransactionBuilder::default()
        .cell_dep(cell_dep)
        .input(input1.clone())
        .input(input2.clone())
        .output(output1)
        .output(output2)
        .output_data(Default::default())
        .output_data(Default::default())
        .build();

    let tx_hash: H256 = tx.hash().unpack();
    // sign input1
    let witness = {
        WitnessArgs::new_builder()
            .lock(Some(Bytes::from(vec![0u8; 65])).pack())
            .build()
    };
    let witness_len: u64 = witness.as_bytes().len() as u64;
    let mut hasher = new_blake2b();
    hasher.update(tx_hash.as_bytes());
    hasher.update(&witness_len.to_le_bytes());
    hasher.update(&witness.as_bytes());
    let message = {
        let mut buf = [0u8; 32];
        hasher.finalize(&mut buf);
        H256::from(buf)
    };
    let sig = privkey.sign_recoverable(&message).expect("sign");
    let witness = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(sig.serialize())).pack())
        .build();
    // sign input2
    let witness2 = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build();
    let witness2_len: u64 = witness2.as_bytes().len() as u64;
    let mut hasher = new_blake2b();
    hasher.update(tx_hash.as_bytes());
    hasher.update(&witness2_len.to_le_bytes());
    hasher.update(&witness2.as_bytes());
    let message2 = {
        let mut buf = [0u8; 32];
        hasher.finalize(&mut buf);
        H256::from(buf)
    };
    let sig2 = privkey2.sign_recoverable(&message2).expect("sign");
    let witness2 = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(sig2.serialize())).pack())
        .build();
    let tx = tx
        .as_advanced_builder()
        .witness(witness.as_bytes().pack())
        .witness(witness2.as_bytes().pack())
        .build();

    let serialized_size = tx.data().as_slice().len() as u64;

    assert_eq!(
        serialized_size, TWO_IN_TWO_OUT_BYTES,
        "2 in 2 out tx serialized size changed, PLEASE UPDATE consensus"
    );

    let (secp256k1_blake160_cell, secp256k1_blake160_cell_data) =
        secp256k1_blake160_sighash_cell(consensus.clone());

    let (secp256k1_data_cell, secp256k1_data_cell_data) = secp256k1_data_cell(consensus);

    let input_cell1 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock)
        .build();

    let resolved_input_cell1 = CellMetaBuilder::from_cell_output(input_cell1, Default::default())
        .out_point(input1.previous_output())
        .build();

    let input_cell2 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock2)
        .build();

    let resolved_input_cell2 = CellMetaBuilder::from_cell_output(input_cell2, Default::default())
        .out_point(input2.previous_output())
        .build();

    let resolved_secp256k1_blake160_cell =
        CellMetaBuilder::from_cell_output(secp256k1_blake160_cell, secp256k1_blake160_cell_data)
            .build();

    let resolved_secp_data_cell =
        CellMetaBuilder::from_cell_output(secp256k1_data_cell, secp256k1_data_cell_data).build();

    ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![resolved_secp256k1_blake160_cell, resolved_secp_data_cell],
        resolved_inputs: vec![resolved_input_cell1, resolved_input_cell2],
        resolved_dep_groups: vec![],
    }
}
