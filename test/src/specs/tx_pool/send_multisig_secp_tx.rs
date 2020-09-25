use crate::util::check::is_transaction_committed;
use crate::{Node, Spec};
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::{build_genesis_type_id_script, OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL};
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::JsonBytes;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_MULTISIG_ALL;
use ckb_types::{
    bytes::Bytes,
    core::{capacity_bytes, Capacity, DepType, ScriptHashType, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint, WitnessArgs},
    prelude::*,
    H160, H256,
};
use log::info;

pub struct SendMultiSigSecpTxUseDepGroup {
    // secp lock script's hash type
    hash_type: ScriptHashType,
    keys: Vec<Privkey>,
    name: &'static str,
}

impl SendMultiSigSecpTxUseDepGroup {
    pub fn new(name: &'static str, hash_type: ScriptHashType) -> Self {
        let keys = vec![Generator::random_privkey(); 3];
        SendMultiSigSecpTxUseDepGroup {
            name,
            hash_type,
            keys,
        }
    }
}

impl Spec for SendMultiSigSecpTxUseDepGroup {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash(), 1);
        let block = node.get_tip_block();
        let cellbase_hash = block.transactions()[0].hash();

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.into())
            .build();
        let output = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(node.always_success_script())
            .build();
        let input = CellInput::new(OutPoint::new(cellbase_hash, 0), 0);
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep.clone())
            .input(input.clone())
            .output(output.clone())
            .output_data(Default::default())
            .build();
        let multi_sign_script = gen_multi_sign_script(&self.keys, self.keys.len() as u8, 0);
        let tx_hash = tx.hash();
        let witness = {
            let mut lock = multi_sign_script.to_vec();
            lock.extend(vec![0u8; 65 * self.keys.len()]);
            WitnessArgs::new_builder()
                .lock(Some(Bytes::from(lock)).pack())
                .build()
        };
        let witness_len = witness.as_slice().len() as u64;
        let message = {
            let mut hasher = new_blake2b();
            hasher.update(&tx_hash.as_slice());
            hasher.update(&witness_len.to_le_bytes());
            hasher.update(&witness.as_slice());
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        // sign
        let mut lock = multi_sign_script.to_vec();
        self.keys.iter().for_each(|key| {
            let sig = key.sign_recoverable(&message).expect("sign");
            lock.extend_from_slice(&sig.serialize());
        });
        let witness = witness
            .as_builder()
            .lock(Some(Bytes::from(lock)).pack())
            .build();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep)
            .input(input)
            .output(output)
            .output_data(Default::default())
            .witness(witness.as_bytes().pack())
            .build();
        info!("Send 1 multisig tx use dep group");

        node.rpc_client().send_transaction(tx.data().into());
        node.generate_blocks(20);

        assert!(is_transaction_committed(node, &tx));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let multi_sign_script = gen_multi_sign_script(&self.keys, self.keys.len() as u8, 0);
        let lock_arg = Bytes::from(blake160(&multi_sign_script).as_bytes().to_vec());
        let hash_type = self.hash_type;
        let block_assembler = new_block_assembler_config(lock_arg, hash_type);
        config.block_assembler = Some(block_assembler);
    }
}

fn blake160(data: &[u8]) -> H160 {
    let result = blake2b_256(data);
    H160::from_slice(&result[..20]).unwrap()
}

fn gen_multi_sign_script(keys: &[Privkey], threshold: u8, require_first_n: u8) -> Bytes {
    let pubkeys = keys
        .iter()
        .map(|key| key.pubkey().unwrap())
        .collect::<Vec<_>>();
    let mut script = vec![0u8, require_first_n, threshold, pubkeys.len() as u8];
    pubkeys.iter().for_each(|pubkey| {
        script.extend_from_slice(&blake160(&pubkey.serialize()).as_bytes());
    });
    script.into()
}

fn type_lock_script_code_hash() -> H256 {
    build_genesis_type_id_script(OUTPUT_INDEX_SECP256K1_BLAKE160_MULTISIG_ALL)
        .calc_script_hash()
        .unpack()
}

fn new_block_assembler_config(lock_arg: Bytes, hash_type: ScriptHashType) -> BlockAssemblerConfig {
    let code_hash = if hash_type == ScriptHashType::Data {
        CODE_HASH_SECP256K1_BLAKE160_MULTISIG_ALL.clone()
    } else {
        type_lock_script_code_hash()
    };
    BlockAssemblerConfig {
        code_hash,
        hash_type: hash_type.into(),
        args: JsonBytes::from_bytes(lock_arg),
        message: Default::default(),
    }
}
