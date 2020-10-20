use super::{new_block_assembler_config, type_lock_script_code_hash};
use crate::util::check::is_transaction_committed;
use crate::{Node, Spec};
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_types::{
    bytes::Bytes,
    core::{capacity_bytes, Capacity, DepType, ScriptHashType, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
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

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash(), 0);
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
        let witness = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(vec![0u8; 65])).pack())
            .build();
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
        let sig = self.privkey.sign_recoverable(&message).expect("sign");
        let witness = witness
            .as_builder()
            .lock(Some(Bytes::from(sig.serialize())).pack())
            .build();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep)
            .input(input)
            .output(output)
            .output_data(Default::default())
            .witness(witness.as_bytes().pack())
            .build();
        info!("Send 1 secp tx use dep group");

        node.rpc_client().send_transaction(tx.data().into());
        node.generate_blocks(20);

        assert!(is_transaction_committed(node, &tx));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let pubkey_data = self
            .privkey
            .pubkey()
            .expect("Get pubkey failed")
            .serialize();
        let lock_arg = Bytes::from(blake2b_256(&pubkey_data)[0..20].to_vec());
        let hash_type = self.hash_type;
        let block_assembler = new_block_assembler_config(lock_arg, hash_type);
        config.block_assembler = Some(block_assembler);
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
        let lock_arg = Bytes::from(blake2b_256(&pubkey_data)[0..20].to_vec());
        CheckTypical2In2OutTx { privkey, lock_arg }
    }
}

impl Spec for CheckTypical2In2OutTx {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash(), 0);

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
            .lock(lock)
            .build();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep)
            .input(input1)
            .input(input2)
            .output(output1)
            .output(output2)
            .output_data(Default::default())
            .output_data(Default::default())
            .build();

        let tx_hash: H256 = tx.hash().unpack();
        let witness = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(vec![0u8; 65])).pack())
            .build();
        let witness_len = witness.as_slice().len() as u64;
        let witness2 = Bytes::new();
        let witness2_len = witness2.len() as u64;
        let message = {
            let mut hasher = new_blake2b();
            hasher.update(&tx_hash.as_bytes());
            hasher.update(&witness_len.to_le_bytes());
            hasher.update(&witness.as_slice());
            hasher.update(&witness2_len.to_le_bytes());
            hasher.update(&witness2);
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        let sig = self.privkey.sign_recoverable(&message).expect("sign");
        let witness = witness
            .as_builder()
            .lock(Some(Bytes::from(sig.serialize())).pack())
            .build();
        let tx = tx
            .as_advanced_builder()
            .witness(witness.as_bytes().pack())
            .witness(witness2.pack())
            .build();

        info!("Send 1 secp tx use dep group");
        node.rpc_client()
            .inner()
            .send_transaction(tx.data().into(), None)
            .expect("should pass default outputs validator")
            .pack();
        node.generate_blocks(20);

        assert!(is_transaction_committed(node, &tx));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.lock_arg.clone();
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}
