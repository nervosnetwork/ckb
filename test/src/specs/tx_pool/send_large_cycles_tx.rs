use super::{new_block_assembler_config, type_lock_script_code_hash};
use crate::util::transaction::relay_tx;
use crate::utils::wait_until;
use crate::{Net, Node, Spec};
use ckb_crypto::secp::{Generator, Privkey};
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_jsonrpc_types::Status;
use ckb_logger::info;
use ckb_network::SupportProtocols;
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

pub struct SendLargeCyclesTxInBlock {
    random_key: RandomKey,
}

impl SendLargeCyclesTxInBlock {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        SendLargeCyclesTxInBlock {
            random_key: RandomKey::new(),
        }
    }
}

impl Spec for SendLargeCyclesTxInBlock {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node1.mine_until_out_bootstrap_period();
        info!("Generate large cycles tx");
        let tx = build_tx(node1, &self.random_key.privkey, self.random_key.lock_arg());

        info!("Node0 mine large cycles tx");
        node0.connect(node1);
        let result = wait_until(60, || {
            node1.get_tip_block_number() == node0.get_tip_block_number()
        });
        assert!(result, "node0 can't sync with node1");
        node0.disconnect(node1);
        let ret = node0.rpc_client().send_transaction_result(tx.data().into());
        ret.expect("package large cycles tx");
        let result = wait_until(60, || {
            let ret = node0
                .rpc_client()
                .get_transaction_with_verbosity(tx.hash(), 1);
            matches!(ret.tx_status.status, Status::Pending)
        });
        assert!(result, "large cycles tx rejected by node0");
        node0.mine_until_transaction_confirm(&tx.hash());
        let block: BlockView = node0.get_tip_block();
        assert_eq!(block.transactions()[1], tx);
        node0.connect(node1);

        info!("Wait block relay to node1");
        let result = wait_until(60, || {
            let block2: BlockView = node1.get_tip_block();
            block2.hash() == block.hash()
        });
        assert!(result, "block can't relay to node1");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.random_key.lock_arg();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 5000u64;
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

pub struct SendLargeCyclesTxToRelay {
    random_key: RandomKey,
}

impl SendLargeCyclesTxToRelay {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        SendLargeCyclesTxToRelay {
            random_key: RandomKey::new(),
        }
    }
}

impl Spec for SendLargeCyclesTxToRelay {
    crate::setup!(num_nodes: 2, retry_failed: 5);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        node1.mine_until_out_bootstrap_period();
        node0.connect(node1);
        info!("Generate large cycles tx");

        let tx = build_tx(node1, &self.random_key.privkey, self.random_key.lock_arg());
        // send tx
        let ret = node1.rpc_client().send_transaction_result(tx.data().into());
        assert!(ret.is_ok());

        info!("Node1 submit large cycles tx");

        let result = wait_until(60, || {
            node1.get_tip_block_number() == node0.get_tip_block_number()
        });
        assert!(result, "node0 can't sync with node1");

        let result = wait_until(60, || {
            node0
                .rpc_client()
                .get_transaction(tx.hash())
                .transaction
                .is_some()
        });
        assert!(result, "Node0 should accept tx");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.random_key.lock_arg();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 5000u64;
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

pub struct NotifyLargeCyclesTx {
    random_key: RandomKey,
}

impl NotifyLargeCyclesTx {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        NotifyLargeCyclesTx {
            random_key: RandomKey::new(),
        }
    }
}

impl Spec for NotifyLargeCyclesTx {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        info!("Generate large cycles tx");
        let tx = build_tx(node0, &self.random_key.privkey, self.random_key.lock_arg());
        // send tx
        let _ = node0.rpc_client().notify_transaction(tx.data().into());

        info!("Node0 receive notify large cycles tx");

        let result = wait_until(60, || {
            node0
                .rpc_client()
                .get_transaction(tx.hash())
                .transaction
                .is_some()
        });
        assert!(result, "Node0 should accept tx");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.random_key.lock_arg();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 13_000u64; // transferred_byte_cycles 12678
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

pub struct LoadProgramFailedTx {
    random_key: RandomKey,
}

impl LoadProgramFailedTx {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        LoadProgramFailedTx {
            random_key: RandomKey::new(),
        }
    }
}

impl Spec for LoadProgramFailedTx {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        info!("Generate large cycles tx");
        let tx = build_tx(node0, &self.random_key.privkey, self.random_key.lock_arg());
        // send tx
        let _ = node0.rpc_client().notify_transaction(tx.data().into());

        info!("Node0 receive notify large cycles tx");

        let result = wait_until(60, || {
            node0
                .rpc_client()
                .get_transaction(tx.hash())
                .transaction
                .is_some()
        });
        assert!(result, "Node0 should accept tx");
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.random_key.lock_arg();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 1_300u64; // transferred_byte_cycles 12678
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

pub struct RelayWithWrongTx {
    random_key: RandomKey,
}

impl RelayWithWrongTx {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        RelayWithWrongTx {
            random_key: RandomKey::new(),
        }
    }
}

impl Spec for RelayWithWrongTx {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];

        node0.mine_until_out_bootstrap_period();
        let rpc_client = node0.rpc_client();

        let tx = build_tx(node0, &self.random_key.privkey, self.random_key.lock_arg());

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::RelayV2, SupportProtocols::Sync],
        );
        net.connect(node0);

        relay_tx(&net, node0, tx, 100_000_000);
        let ret = wait_until(10, || {
            let peers = rpc_client.get_peers();
            peers.is_empty()
        });
        assert!(
            ret,
            "The address of net should be removed from node0's peers",
        );
        rpc_client.clear_banned_addresses();

        // Advance one block, in order to prevent tx hash is same
        node0.mine(1);

        let mut generator = Generator::new();
        let tx_wrong_pk = build_tx(node0, &generator.gen_privkey(), self.random_key.lock_arg());

        net.connect(node0);

        relay_tx(&net, node0, tx_wrong_pk, 100_000_000);
        let ret = wait_until(10, || {
            let peers = rpc_client.get_peers();
            peers.is_empty()
        });
        assert!(
            ret,
            "The address of net should be removed from node0's peers",
        );
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        let lock_arg = self.random_key.lock_arg();
        config.network.connect_outbound_interval_secs = 0;
        config.tx_pool.max_tx_verify_cycles = 1_300u64; // transferred_byte_cycles 12678
        let block_assembler = new_block_assembler_config(lock_arg, ScriptHashType::Type);
        config.block_assembler = Some(block_assembler);
    }
}

struct RandomKey {
    privkey: Privkey,
    lock_arg: Bytes,
}

impl RandomKey {
    #[allow(clippy::new_without_default)]
    fn new() -> Self {
        let privkey = Generator::new().gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from(blake2b_256(&pubkey_data)[0..20].to_vec());
        Self { privkey, lock_arg }
    }

    fn lock_arg(&self) -> Bytes {
        self.lock_arg.clone()
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
        hasher.update(tx_hash.as_bytes());
        hasher.update(&witness_len.to_le_bytes());
        hasher.update(witness.as_slice());
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
