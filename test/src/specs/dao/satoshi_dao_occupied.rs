use super::*;
use crate::utils::is_committed;
use crate::{Net, Spec};
use ckb_chain_spec::{ChainSpec, IssuedCell};
use ckb_crypto::secp::{Generator, Privkey, Pubkey};
use ckb_dao_utils::extract_dao_data;
use ckb_hash::new_blake2b;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    bytes::Bytes,
    constants::TYPE_ID_CODE_HASH,
    core::{Capacity, DepType, Ratio},
    prelude::*,
    H160, H256,
};

const SATOSHI_CELL_CAPACITY: Capacity = Capacity::shannons(10_000_000_000_000_000);
const CELLBASE_USED_BYTES: usize = 41;

pub struct DAOWithSatoshiCellOccupied;

impl Spec for DAOWithSatoshiCellOccupied {
    crate::name!("dao_with_satoshi_cell_occupied");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        // try deposit then withdraw dao
        node0.generate_blocks(2);
        let deposited = {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction)
        };
        let transaction = withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone());
        node0.generate_blocks(20);
        let tx_hash = node0
            .rpc_client()
            .send_transaction(transaction.data().into());
        node0.generate_blocks(3);
        let tx_status = node0
            .rpc_client()
            .get_transaction(tx_hash.clone())
            .expect("get sent transaction");
        assert!(
            is_committed(&tx_status),
            "ensure_committed failed {:#x}",
            tx_hash
        );
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|spec_config| {
            let satoshi_cell = issue_satoshi_cell(H160([0u8; 20]));
            spec_config.genesis.issued_cells.push(satoshi_cell);
        })
    }
}

pub struct SpendSatoshiCell {
    privkey: Privkey,
    pubkey: Pubkey,
    satoshi_pubkey_hash: H160,
    satoshi_cell_occupied_ratio: Ratio,
}

impl Default for SpendSatoshiCell {
    fn default() -> Self {
        Self::new()
    }
}

impl SpendSatoshiCell {
    pub fn new() -> Self {
        let (privkey, pubkey) = Generator::random_keypair();
        let satoshi_pubkey_hash = pubkey_hash160(&pubkey.serialize());
        let satoshi_cell_occupied_ratio = Ratio(6, 10);

        SpendSatoshiCell {
            privkey,
            pubkey,
            satoshi_pubkey_hash,
            satoshi_cell_occupied_ratio,
        }
    }
}

impl Spec for SpendSatoshiCell {
    crate::name!("spend_satoshi_cell");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        let satoshi_cell_occupied = SATOSHI_CELL_CAPACITY
            .safe_mul_ratio(node0.consensus().satoshi_cell_occupied_ratio)
            .unwrap();
        // check genesis blocks dao
        let genesis = node0.get_block_by_number(0);
        let (_ar, _c, u) = extract_dao_data(genesis.header().dao()).expect("extract dao");
        // u - used capacity should includes virtual occupied
        assert!(u > satoshi_cell_occupied);

        // Build tx to spent virtual occupied capacity
        let cellbase = &genesis.transactions()[0];
        let satoshi_input = CellInput::new(
            OutPoint::new(cellbase.hash(), (cellbase.outputs().len() - 1) as u32),
            0,
        );
        let secp_out_point = OutPoint::new(node0.dep_group_tx_hash().clone(), 1);
        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.pack())
            .build();
        let output = CellOutput::new_builder()
            .capacity(satoshi_cell_occupied.pack())
            .lock(always_success_cell().2.clone())
            .build();

        let transaction = TransactionBuilder::default()
            .cell_deps(vec![cell_dep])
            .input(satoshi_input)
            .output(output)
            .output_data(Bytes::new().pack())
            .build();
        let tx_hash = transaction.hash();
        let sig = self
            .privkey
            .sign_recoverable(&tx_hash.unpack())
            .expect("sign");
        let witness = vec![
            Bytes::from(sig.serialize()).pack(),
            Bytes::from(self.pubkey.serialize()).pack(),
        ]
        .pack();
        let transaction = transaction.as_advanced_builder().witness(witness).build();

        node0.generate_blocks(1);
        let tx_hash = node0
            .rpc_client()
            .send_transaction(transaction.data().into());
        node0.generate_blocks(3);
        // cellbase occupied capacity minus satoshi cell
        let cellbase_used_capacity =
            Capacity::bytes(CELLBASE_USED_BYTES * node0.spec().genesis.system_cells.len()).unwrap();
        let tx_status = node0
            .rpc_client()
            .get_transaction(tx_hash.clone())
            .expect("get sent transaction");
        assert!(
            is_committed(&tx_status),
            "ensure_committed failed {:#x}",
            tx_hash
        );
        let tip = node0.get_tip_block();
        // check tip dao, expect u correct
        let (_ar, _c, new_u) = extract_dao_data(tip.header().dao()).expect("extract dao");
        assert_eq!(
            Ok(new_u),
            u.safe_sub(satoshi_cell_occupied)
                .and_then(|c| c.safe_add(cellbase_used_capacity))
        );
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        let satoshi_pubkey_hash = self.satoshi_pubkey_hash.clone();
        let satoshi_cell_occupied_ratio = self.satoshi_cell_occupied_ratio;
        Box::new(move |spec_config| {
            spec_config
                .genesis
                .issued_cells
                .push(issue_satoshi_cell(satoshi_pubkey_hash.clone()));
            spec_config.genesis.satoshi_gift.satoshi_pubkey_hash = satoshi_pubkey_hash.clone();
            spec_config.genesis.satoshi_gift.satoshi_cell_occupied_ratio =
                satoshi_cell_occupied_ratio;
        })
    }
}

fn issue_satoshi_cell(satoshi_pubkey_hash: H160) -> IssuedCell {
    let lock = Script::new_builder()
        .args(vec![Bytes::from(&satoshi_pubkey_hash.0[..]).pack()].pack())
        .code_hash(type_lock_script_code_hash().pack())
        .hash_type(ScriptHashType::Type.pack())
        .build();
    IssuedCell {
        capacity: SATOSHI_CELL_CAPACITY,
        lock: lock.into(),
    }
}

fn type_lock_script_code_hash() -> H256 {
    let input = CellInput::new_cellbase_input(0);
    // 0 => genesis cell, which contains a message and can never be spent.
    // 1 => always success cell
    // ....
    // 5 => secp256k1_ripemd160_sha256_sighash_all cell
    // define in integration.toml spec file
    let output_index: u64 = 5;
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

fn pubkey_hash160(data: &[u8]) -> H160 {
    fn ripemd160(data: &[u8]) -> H160 {
        use ripemd160::{Digest, Ripemd160};
        let digest = Ripemd160::digest(data);
        H160(digest.into())
    }

    fn sha256(data: &[u8]) -> H256 {
        use sha2::{Digest, Sha256};
        let digest: [u8; 32] = Sha256::digest(data).into();
        H256(digest)
    }
    ripemd160(sha256(data).as_bytes())
}
