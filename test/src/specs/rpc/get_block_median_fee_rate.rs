use crate::{Node, Spec};
// use crate::util::cell::gen_spendable;
// use crate::util::transaction::always_success_transaction;
use crate::util::mining::{mine, mine_until_out_bootstrap_period};
use ckb_app_config::BlockAssemblerConfig;
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::JsonBytes;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;
use ckb_test_chain_utils::type_lock_script_code_hash;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, DepType, ScriptHashType, TransactionBuilder, TransactionView},
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
    prelude::*,
    H256,
};

const N_BLOCKS: u64 = 10;
const N_TXS_PER_BLOCK: u64 = 10;
const BASE_CAPACITY: usize = 0x16b969d00;

pub struct RpcGetBlockMedianFeeRate {
    privkey: Privkey,
    lock_arg: Bytes,
}

impl RpcGetBlockMedianFeeRate {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let mut generator = Generator::new();
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from(blake2b_256(&pubkey_data)[0..20].to_vec());
        RpcGetBlockMedianFeeRate { privkey, lock_arg }
    }
}

impl Spec for RpcGetBlockMedianFeeRate {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        mine_until_out_bootstrap_period(node0);

        let tip = node0.get_tip_block().number();

        // mine enough blocks for valid input cell ready
        let number_transactions = N_TXS_PER_BLOCK * N_BLOCKS;
        mine(&node0, number_transactions);

        // build enough txs
        let txs = build_tx(
            &node0,
            &self.privkey,
            self.lock_arg.clone(),
            tip,
            number_transactions,
        );

        // put N_TXS_PER_BLOCK txs into each block and commit N_BLOCKS blocks
        for chunk in txs.chunks(N_TXS_PER_BLOCK as usize).into_iter() {
            for tx in chunk {
                node0
                    .rpc_client()
                    .send_transaction_result(tx.data().into())
                    .expect("package large cycles tx");
            }
            mine(&node0, 1);
        }

        // test with user specific blocks_to_scan and transactions number
        let result = node0.rpc_client().get_median_fee_rate(Some(10), Some(1));
        assert!(result.is_ok());
        // println!("result value:{}", result.unwrap().value());

        // test with default
        let result = node0.rpc_client().get_median_fee_rate(None, None);
        assert!(result.is_ok());

        // test with "not enough transactions in 1 block"
        let result = node0
            .rpc_client()
            .get_median_fee_rate(Some(1), Some(N_TXS_PER_BLOCK + 1));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("InvalidParams: chain hasn't enough transactions in 1 recent blocks"));
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.lock_arg.clone();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 1_300u64;
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

fn build_tx(
    node: &Node,
    privkey: &Privkey,
    lock_arg: Bytes,
    base_block_index: u64,
    count: u64,
) -> Vec<TransactionView> {
    let mut tx_vec = Vec::with_capacity(count as usize);

    for index in 0..count {
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
            // let block = node.get_tip_block();
            let block = node.get_block_by_number(base_block_index + index as u64);
            let cellbase_hash = block.transactions()[0].hash();
            CellInput::new(OutPoint::new(cellbase_hash, 0), 0)
        };
        let output1 = CellOutput::new_builder()
            // .capacity(capacity_bytes!(out_cell_capacity).pack())
            .capacity(Capacity::shannons(BASE_CAPACITY as u64).pack())
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
        tx_vec.push(
            tx.as_advanced_builder()
                .witness(witness.as_bytes().pack())
                .build(),
        )
    }
    tx_vec
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
        use_binary_version_as_message_prefix: false,
        binary_version: "TEST".to_string(),
    }
}
