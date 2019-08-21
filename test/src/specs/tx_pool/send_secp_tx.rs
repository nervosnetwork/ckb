use crate::utils::is_committed;
use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::blake2b_256;
use ckb_jsonrpc_types::JsonBytes;
use ckb_miner::BlockAssemblerConfig;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;
use ckb_types::{
    bytes::Bytes,
    core::{capacity_bytes, Capacity, ScriptHashType, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
    H256,
};
use log::info;

pub struct SendSecpTxUseDepGroup {
    privkey: Privkey,
}

impl Default for SendSecpTxUseDepGroup {
    fn default() -> Self {
        let privkey = Generator::new().random_privkey();
        SendSecpTxUseDepGroup { privkey }
    }
}

impl Spec for SendSecpTxUseDepGroup {
    crate::name!("send_secp_tx_use_dep_group");

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        info!("Generate 20 block on node");
        node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash().clone(), 0);
        let block = node.get_tip_block();
        let cellbase_hash: H256 = block.transactions()[0].hash().to_owned().unpack();

        let cell_dep = CellDep::new(secp_out_point, true);
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

        let tx_hash: H256 = tx.hash().unpack();
        let message = H256::from(blake2b_256(&tx_hash));
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
            "ensure_committed failed {:#x}",
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
        Box::new(move |config| {
            let block_assembler = BlockAssemblerConfig {
                code_hash: CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL.clone(),
                hash_type: ScriptHashType::Data.into(),
                args: vec![JsonBytes::from_bytes(lock_arg.clone())],
                data: Default::default(),
            };
            config.block_assembler = Some(block_assembler);
        })
    }
}
