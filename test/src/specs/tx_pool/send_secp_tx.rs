use crate::utils::is_committed;
use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_chain_spec::{build_genesis_type_id_script, OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL};
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::blake2b_256;
use ckb_jsonrpc_types::JsonBytes;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;
use ckb_tx_pool::BlockAssemblerConfig;
use ckb_types::{
    bytes::Bytes,
    core::{capacity_bytes, Capacity, DepType, ScriptHashType, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
    H256,
};
use log::info;

pub struct SendSecpTxUseDepGroup {
    // secp lock script's hash type
    hash_type: ScriptHashType,
    privkey: Privkey,
    name: &'static str,
}

impl SendSecpTxUseDepGroup {
    pub fn new(name: &'static str, hash_type: ScriptHashType) -> Self {
        let privkey = Generator::random_privkey();
        SendSecpTxUseDepGroup {
            name,
            hash_type,
            privkey,
        }
    }
}

impl Spec for SendSecpTxUseDepGroup {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash().clone(), 0);
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

        let tx_hash = tx.hash();
        let message = H256::from(blake2b_256(tx_hash.as_slice()));
        let sig = self.privkey.sign_recoverable(&message).expect("sign");
        let witness = Bytes::from(sig.serialize()).pack();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep)
            .input(input)
            .output(output)
            .output_data(Default::default())
            .witness(witness)
            .build();
        info!("Send 1 secp tx use dep group");

        let tx_hash = node.rpc_client().send_transaction(tx.data().into());
        node.generate_blocks(20);

        let tx_status = node
            .rpc_client()
            .get_transaction(tx_hash.clone())
            .expect("get sent transaction");
        assert!(
            is_committed(&tx_status),
            "ensure_committed failed {}",
            tx_hash
        );
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        let pubkey_data = self
            .privkey
            .pubkey()
            .expect("Get pubkey failed")
            .serialize();
        let lock_arg = Bytes::from(&blake2b_256(&pubkey_data)[0..20]);
        let hash_type = self.hash_type;
        Box::new(move |config| {
            let block_assembler = new_block_assembler_config(lock_arg.clone(), hash_type);
            config.block_assembler = Some(block_assembler);
        })
    }
}

pub struct CheckTypical2In2OutTx {
    privkey: Privkey,
    lock_arg: Bytes,
}

impl Default for CheckTypical2In2OutTx {
    fn default() -> Self {
        Self::new(42)
    }
}

impl CheckTypical2In2OutTx {
    pub fn new(seed: u64) -> Self {
        let mut generator = Generator::non_crypto_safe_prng(seed);
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from(&blake2b_256(&pubkey_data)[0..20]);
        CheckTypical2In2OutTx { privkey, lock_arg }
    }
}

impl Spec for CheckTypical2In2OutTx {
    crate::name!("check_typical_2_in_2_out_tx");

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash().clone(), 0);

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.into())
            .build();
        let input1 = {
            let block = node.get_tip_block();
            let cellbase_hash = block.transactions()[0].hash();
            CellInput::new(OutPoint::new(cellbase_hash, 0), 0)
        };
        node.generate_blocks(1);
        let input2 = {
            let block = node.get_tip_block();
            let cellbase_hash = block.transactions()[0].hash();
            CellInput::new(OutPoint::new(cellbase_hash, 0), 0)
        };
        let lock = Script::new_builder()
            .args(self.lock_arg.pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(ScriptHashType::Type.into())
            .build();
        let output1 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock.clone())
            .build();
        let output2 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock.clone())
            .build();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep.clone())
            .input(input1.clone())
            .input(input2.clone())
            .output(output1.clone())
            .output(output2.clone())
            .output_data(Default::default())
            .output_data(Default::default())
            .build();

        let tx_hash: H256 = tx.hash().unpack();
        let message = H256::from(blake2b_256(&tx_hash));
        let sig = self.privkey.sign_recoverable(&message).expect("sign");
        let witness = Bytes::from(sig.serialize()).pack();
        let tx = tx
            .as_advanced_builder()
            .witness(witness.clone())
            .witness(witness.clone())
            .build();

        info!("Send 1 secp tx use dep group");
        let tx_hash = node.rpc_client().send_transaction(tx.data().into());
        node.generate_blocks(20);

        let tx_status = node
            .rpc_client()
            .get_transaction(tx_hash.clone())
            .expect("get sent transaction");
        assert!(
            is_committed(&tx_status),
            "ensure_committed failed {:#x}",
            tx_hash
        );
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        let lock_arg = self.lock_arg.clone();
        Box::new(move |config| {
            let block_assembler =
                new_block_assembler_config(lock_arg.clone(), ScriptHashType::Type);
            config.block_assembler = Some(block_assembler);
        })
    }
}

fn type_lock_script_code_hash() -> H256 {
    build_genesis_type_id_script(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL)
        .calc_script_hash()
        .unpack()
}

fn new_block_assembler_config(lock_arg: Bytes, hash_type: ScriptHashType) -> BlockAssemblerConfig {
    let code_hash = if hash_type == ScriptHashType::Data {
        CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL.clone()
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
