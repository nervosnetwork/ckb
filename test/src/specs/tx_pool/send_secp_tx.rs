use crate::utils::is_committed;
use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::JsonBytes;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;
use ckb_tx_pool::BlockAssemblerConfig;
use ckb_types::{
    bytes::Bytes,
    constants::TYPE_ID_CODE_HASH,
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
            let code_hash = if hash_type == ScriptHashType::Data {
                CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL.clone()
            } else {
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
            };
            let block_assembler = BlockAssemblerConfig {
                code_hash,
                hash_type: hash_type.into(),
                args: vec![JsonBytes::from_bytes(lock_arg.clone())],
                data: Default::default(),
            };
            config.block_assembler = Some(block_assembler);
        })
    }
}
