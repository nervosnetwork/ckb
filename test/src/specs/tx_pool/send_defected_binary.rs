use super::new_block_assembler_config;
use crate::utils::is_committed;
use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_types::{
    bytes::Bytes,
    core::{capacity_bytes, Capacity, DepType, ScriptHashType, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint, WitnessArgs},
    prelude::*,
    H256,
};
use log::info;

pub struct SendDefectedBinary {
    privkey: Privkey,
    name: &'static str,
    reject_ill_transactions: bool,
}

impl SendDefectedBinary {
    pub fn new(name: &'static str, reject_ill_transactions: bool) -> Self {
        let privkey = Generator::random_privkey();
        SendDefectedBinary {
            name,
            privkey,
            reject_ill_transactions,
        }
    }
}

impl Spec for SendDefectedBinary {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];

        info!("Generate 20 blocks to work around initial blocks without rewards");
        node.generate_blocks(20);

        info!("Generate 20 blocks on node");
        let hashes = node.generate_blocks(20);

        let secp_out_point = OutPoint::new(node.dep_group_tx_hash(), 0);
        let inputs = hashes.into_iter().map(|hash| {
            let block = node.get_block(hash);
            let cellbase_hash = block.transactions()[0].hash();
            CellInput::new(OutPoint::new(cellbase_hash, 0), 0)
        });

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.into())
            .build();
        let output = CellOutput::new_builder()
            .capacity(capacity_bytes!(5000).pack())
            .lock(node.always_success_script())
            .build();
        let data = include_bytes!("../../../../script/testdata/defected_binary");
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep.clone())
            .inputs(inputs.clone())
            .output(output.clone())
            .output_data(data[..].pack())
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
            .inputs(inputs)
            .output(output)
            .output_data(data[..].pack())
            .witness(witness.as_bytes().pack())
            .build();
        info!("Send 1 secp tx with defected binary");

        let ret = node.rpc_client().send_transaction_result(tx.data().into());

        if self.reject_ill_transactions {
            assert!(ret.is_err());
        } else {
            let tx_hash = ret.expect("transaction should be accepted").pack();
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
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        let pubkey_data = self
            .privkey
            .pubkey()
            .expect("Get pubkey failed")
            .serialize();
        let lock_arg = Bytes::from(&blake2b_256(&pubkey_data)[0..20]);
        let reject_ill_transactions = self.reject_ill_transactions;
        Box::new(move |config| {
            let block_assembler =
                new_block_assembler_config(lock_arg.clone(), ScriptHashType::Type);
            config.block_assembler = Some(block_assembler);
            config.rpc.reject_ill_transactions = reject_ill_transactions;
        })
    }
}
