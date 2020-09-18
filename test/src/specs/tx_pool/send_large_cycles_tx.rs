use super::{new_block_assembler_config, type_lock_script_code_hash};
use crate::utils::wait_until;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};

use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockView, Capacity, DepType, ScriptHashType, TransactionBuilder,
        TransactionView,
    },
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
    prelude::*,
    H256,
};
use log::info;

pub struct SendLargeCyclesTxInBlock {
    privkey: Privkey,
    lock_arg: Bytes,
}

impl SendLargeCyclesTxInBlock {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let mut generator = Generator::new();
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from(blake2b_256(&pubkey_data)[0..20].to_vec());
        SendLargeCyclesTxInBlock { privkey, lock_arg }
    }
}

impl Spec for SendLargeCyclesTxInBlock {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        // high cycle limit node
        let mut node0 = nodes.remove(0);
        // low cycle limit node
        let node1 = &nodes[0];
        node0.stop();
        node0.modify_app_config(|config| {
            config.tx_pool.max_tx_verify_cycles = std::u32::MAX.into();
        });
        node0.start();

        node1.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        info!("Generate large cycles tx");
        let tx = build_tx(&node1, &self.privkey, self.lock_arg.clone());
        // send tx
        let ret = node1.rpc_client().send_transaction_result(tx.data().into());
        assert!(ret.is_err());

        info!("Node0 mine large cycles tx");
        node0.connect(&node1);
        let result = wait_until(60, || {
            node1.get_tip_block_number() == node0.get_tip_block_number()
        });
        assert!(result, "node0 can't sync with node1");
        node0.disconnect(&node1);
        let ret = node0.rpc_client().send_transaction_result(tx.data().into());
        ret.expect("package large cycles tx");
        node0.generate_blocks(3);
        let block: BlockView = node0.get_tip_block();
        assert_eq!(block.transactions()[1], tx);
        node0.connect(&node1);

        info!("Wait block relay to node1");
        let result = wait_until(60, || {
            let block2: BlockView = node1.get_tip_block();
            block2.hash() == block.hash()
        });
        assert!(result, "block can't relay to node1");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.lock_arg.clone();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 5000u64;
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

pub struct SendLargeCyclesTxToRelay {
    privkey: Privkey,
    lock_arg: Bytes,
}

impl SendLargeCyclesTxToRelay {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let mut generator = Generator::new();
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from(blake2b_256(&pubkey_data)[0..20].to_vec());
        SendLargeCyclesTxToRelay { privkey, lock_arg }
    }
}

impl Spec for SendLargeCyclesTxToRelay {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        // high cycle limit node
        let mut node0 = nodes.remove(0);
        // low cycle limit node
        let node1 = &nodes[0];
        node0.stop();
        node0.modify_app_config(|config| {
            config.tx_pool.max_tx_verify_cycles = std::u32::MAX.into();
        });
        node0.start();

        node1.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        info!("Generate large cycles tx");
        let tx = build_tx(&node1, &self.privkey, self.lock_arg.clone());
        // send tx
        let ret = node1.rpc_client().send_transaction_result(tx.data().into());
        assert!(ret.is_err());

        info!("Node0 mine large cycles tx");
        node0.connect(&node1);
        let result = wait_until(60, || {
            node1.get_tip_block_number() == node0.get_tip_block_number()
        });
        assert!(result, "node0 can't sync with node1");
        let ret = node0.rpc_client().send_transaction_result(tx.data().into());
        ret.expect("package large cycles tx");
        info!("Node1 do not accept tx");
        let result = wait_until(5, || {
            node1.rpc_client().get_transaction(tx.hash()).is_some()
        });
        assert!(!result, "Node1 should ignore tx");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.lock_arg.clone();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 5000u64;
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

fn build_tx(node: &Node, privkey: &Privkey, lock_arg: Bytes) -> TransactionView {
    let secp_out_point = OutPoint::new(node.dep_group_tx_hash(), 0);
    let lock = Script::new_builder()
        .args(lock_arg.pack())
        .code_hash(type_lock_script_code_hash().pack())
        .hash_type(ScriptHashType::Type.into())
        .build();
    let cell_dep = CellDep::new_builder()
        .out_point(secp_out_point)
        .dep_type(DepType::DepGroup.into())
        .build();
    let input1 = {
        let block = node.get_tip_block();
        let cellbase_hash = block.transactions()[0].hash();
        CellInput::new(OutPoint::new(cellbase_hash, 0), 0)
    };
    let output1 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock)
        .build();
    let tx = TransactionBuilder::default()
        .cell_dep(cell_dep)
        .input(input1)
        .output(output1)
        .output_data(Default::default())
        .build();
    let tx_hash: H256 = tx.hash().unpack();
    let witness = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build();
    let witness_len = witness.as_slice().len() as u64;
    let message = {
        let mut hasher = new_blake2b();
        hasher.update(&tx_hash.as_bytes());
        hasher.update(&witness_len.to_le_bytes());
        hasher.update(&witness.as_slice());
        let mut buf = [0u8; 32];
        hasher.finalize(&mut buf);
        H256::from(buf)
    };
    let sig = privkey.sign_recoverable(&message).expect("sign");
    let witness = witness
        .as_builder()
        .lock(Some(Bytes::from(sig.serialize())).pack())
        .build();
    tx.as_advanced_builder()
        .witness(witness.as_bytes().pack())
        .build()
}
