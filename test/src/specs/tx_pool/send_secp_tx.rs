use crate::utils::is_committed;
use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::JsonBytes;
use ckb_miner::BlockAssemblerConfig;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;
use ckb_types::{
    bytes::Bytes,
    constants::TYPE_ID_CODE_HASH,
    core::{capacity_bytes, Capacity, Cycle, DepType, ScriptHashType, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script, Witness},
    prelude::*,
    H256,
};
use log::info;

const TX_2_IN_2_OUT_SIZE: usize = 589;
const TX_2_IN_2_OUT_CYCLES: Cycle = 13_334_406;

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

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash().clone(), 0);
        let block = node.get_tip_block();
        let cellbase_hash = block.transactions()[0].hash();

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.pack())
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
        let witness = vec![Bytes::from(sig.serialize()).pack()].pack();
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

    fn run(&self, net: Net) {
        let hash_type = ScriptHashType::Type;

        let node = &net.nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash().clone(), 0);

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.pack())
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
            .args(vec![self.lock_arg.clone()].pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(hash_type.pack())
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
        let witness: Witness = vec![Bytes::from(sig.serialize()).pack()].pack();
        let tx = tx
            .as_advanced_builder()
            .witness(witness.clone())
            .witness(witness.clone())
            .build();
        info!("Check 2 in 2 out tx size");
        let serialized_size = tx.data().as_slice().len();
        assert_eq!(
            serialized_size, TX_2_IN_2_OUT_SIZE,
            "2 in 2 out tx serialized size changed, PLEASE UPDATE consensus"
        );

        info!("Check 2 in 2 out tx cycles");
        let cycles: Cycle = node
            .rpc_client()
            .dry_run_transaction(tx.data().into())
            .cycles
            .into();
        assert_eq!(
            cycles, TX_2_IN_2_OUT_CYCLES,
            "2 in 2 out tx cycles changed, PLEASE UPDATE consensus"
        );

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
    let input = CellInput::new_cellbase_input(0);
    // 0 => genesis cell, which contains a message and can never be spent.
    // 1 => always success cell, define in integration.toml spec file
    let output_index: u64 = 2;
    let mut blake2b = new_blake2b();
    blake2b.update(input.as_slice());
    blake2b.update(&output_index.to_le_bytes());
    let mut ret = [0; 32];
    blake2b.finalize(&mut ret);
    let script_arg = Bytes::from(&ret[..]).pack();
    Script::new_builder()
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.pack())
        .args(vec![script_arg].pack())
        .build()
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
        args: vec![JsonBytes::from_bytes(lock_arg.clone())],
    }
}
