use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_crypto::secp::Privkey;
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_notify::NotifyService;
use ckb_shared::{
    shared::{Shared, SharedBuilder},
    Snapshot,
};
use ckb_store::ChainStore;
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::always_success_cell;
use ckb_traits::chain_provider::ChainProvider;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{resolve_transaction, OverlayCellProvider, TransactionsProvider},
        BlockBuilder, BlockView, Capacity, HeaderView, ScriptHashType, TransactionBuilder,
        TransactionView,
    },
    h160, h256,
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, ProposalShortId, Script},
    prelude::*,
    H160, H256, U256,
};
use lazy_static::lazy_static;
use rand::random;
use std::collections::HashSet;

#[derive(Default)]
pub struct Chains(pub Vec<(ChainController, Shared)>);

impl Chains {
    pub fn push(&mut self, chain: (ChainController, Shared)) {
        self.0.push(chain);
    }
}

pub fn new_always_success_chain(txs_size: usize, chains_num: usize) -> Chains {
    let (_, _, always_success_script) = always_success_cell();
    let tx = create_always_success_tx();
    let dao = genesis_dao_data(&tx).unwrap();

    // create genesis block with N txs
    let transactions: Vec<TransactionView> = (0..txs_size)
        .map(|i| {
            let data = Bytes::from(i.to_le_bytes().to_vec());
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(capacity_bytes!(50_000).pack())
                        .lock(always_success_script.clone())
                        .build(),
                )
                .output_data(data.pack())
                .build()
        })
        .collect();

    let genesis_block = BlockBuilder::default()
        .dao(dao)
        .difficulty(U256::from(1000u64).pack())
        .transaction(tx)
        .transactions(transactions)
        .build();

    let mut consensus = Consensus::default()
        .set_cellbase_maturity(0)
        .set_genesis_block(genesis_block);
    consensus.tx_proposal_window = ProposalWindow(1, 10);

    let mut chains = Chains::default();

    for _ in 0..chains_num {
        let (shared, table) = SharedBuilder::default()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let notify = NotifyService::default().start::<&str>(None);
        let chain_service = ChainService::new(shared.clone(), table, notify);

        chains.push((chain_service.start::<&str>(None), shared));
    }

    chains
}

pub fn create_always_success_tx() -> TransactionView {
    let (ref always_success_cell, ref always_success_cell_data, ref script) = always_success_cell();
    TransactionBuilder::default()
        .witness(script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.pack())
        .build()
}

pub fn create_always_success_cellbase(shared: &Shared, parent: &HeaderView) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();
    let capacity = calculate_reward(shared, parent);
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(parent.number() + 1))
        .output(
            CellOutput::new_builder()
                .capacity(capacity.pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .witness(always_success_script.clone().into_witness())
        .build()
}

pub fn gen_always_success_block(
    blocks: &mut Vec<BlockView>,
    p_block: &BlockView,
    shared: &Shared,
) -> BlockView {
    let tx = create_always_success_tx();
    let always_success_out_point = OutPoint::new(tx.hash().unpack(), 0);
    let (_, _, always_success_script) = always_success_cell();
    let (number, timestamp, difficulty) = (
        p_block.header().number() + 1,
        p_block.header().timestamp() + 10000,
        p_block.header().difficulty() + U256::from(1u64),
    );
    let cellbase = create_always_success_cellbase(shared, &p_block.header());

    // spent n-2 block's tx and proposal n-1 block's tx
    let transactions: Vec<TransactionView> = if blocks.len() > 1 {
        let pp_block = shared
            .store()
            .get_block(&p_block.data().header().raw().parent_hash())
            .expect("gen_block get pp_block");
        pp_block
            .transactions()
            .iter()
            .skip(1)
            .map(|tx| {
                create_transaction(
                    &tx.hash().unpack(),
                    always_success_script.clone(),
                    always_success_out_point.clone(),
                )
            })
            .collect()
    } else {
        vec![]
    };

    let proposals: Vec<ProposalShortId> = p_block
        .transactions()
        .iter()
        .skip(1)
        .map(|tx| {
            create_transaction(
                &tx.hash().unpack(),
                always_success_script.clone(),
                always_success_out_point.clone(),
            )
            .proposal_short_id()
        })
        .collect();

    let mut txs_to_resolve = vec![cellbase.clone()];
    txs_to_resolve.extend_from_slice(&transactions);
    let dao = dao_data(shared, &p_block.header(), &txs_to_resolve);

    let block = BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .proposals(proposals)
        .parent_hash(p_block.hash())
        .number(number.pack())
        .timestamp(timestamp.pack())
        .difficulty(difficulty.pack())
        .nonce(random::<u64>().pack())
        .dao(dao)
        .build();

    blocks.push(block.clone());
    block
}

const PRIVKEY: H256 = h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");

lazy_static! {
    static ref SECP_DATA_CELL: (CellOutput, Bytes) = {
        let raw_data = BUNDLED_CELL
            .get("specs/cells/secp256k1_data")
            .expect("load secp256k1_blake160_sighash_all");
        let data: Bytes = raw_data[..].into();

        let cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        (cell, data)
    };
    static ref SECP_CELL: (CellOutput, Bytes, Script) = {
        let raw_data = BUNDLED_CELL
            .get("specs/cells/secp256k1_blake160_sighash_all")
            .expect("load secp256k1_blake160_sighash_all");
        let data: Bytes = raw_data[..].into();

        let cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();

        let script = Script::new_builder()
            .code_hash(CellOutput::calc_data_hash(&data).pack())
            .args(vec![Bytes::from(PUBKEY_HASH.as_bytes()).pack()].pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();

        (cell, data, script)
    };
}

pub fn secp_cell() -> &'static (CellOutput, Bytes, Script) {
    &SECP_CELL
}

pub fn secp_data_cell() -> &'static (CellOutput, Bytes) {
    &SECP_DATA_CELL
}

pub fn create_secp_tx() -> TransactionView {
    let (ref secp_data_cell, ref secp_data_cell_data) = secp_data_cell();
    let (ref secp_cell, ref secp_cell_data, ref script) = secp_cell();
    let outputs = vec![secp_data_cell.clone(), secp_cell.clone()];
    let outputs_data = vec![secp_data_cell_data.pack(), secp_cell_data.pack()];
    TransactionBuilder::default()
        .witness(script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(outputs)
        .outputs_data(outputs_data)
        .build()
}

pub fn new_secp_chain(txs_size: usize, chains_num: usize) -> Chains {
    let (_, _, secp_script) = secp_cell();
    let tx = create_secp_tx();
    let dao = genesis_dao_data(&tx).unwrap();

    // create genesis block with N txs
    let transactions: Vec<TransactionView> = (0..txs_size)
        .map(|i| {
            let data = Bytes::from(i.to_le_bytes().to_vec());
            let output = CellOutput::new_builder()
                .capacity(capacity_bytes!(50_000).pack())
                .lock(secp_script.clone())
                .build();
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(output.clone())
                .output(output)
                .output_data(data.pack())
                .output_data(data.pack())
                .build()
        })
        .collect();

    let genesis_block = BlockBuilder::default()
        .difficulty(U256::from(1000u64).pack())
        .dao(dao)
        .transaction(tx)
        .transactions(transactions)
        .build();

    let mut consensus = Consensus::default()
        .set_cellbase_maturity(0)
        .set_genesis_block(genesis_block);
    consensus.tx_proposal_window = ProposalWindow(1, 10);

    let mut chains = Chains::default();

    for _ in 0..chains_num {
        let (shared, table) = SharedBuilder::default()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let notify = NotifyService::default().start::<&str>(None);
        let chain_service = ChainService::new(shared.clone(), table, notify);

        chains.push((chain_service.start::<&str>(None), shared));
    }

    chains
}

pub fn create_secp_cellbase(shared: &Shared, parent: &HeaderView) -> TransactionView {
    let (_, _, secp_script) = secp_cell();
    let capacity = calculate_reward(shared, parent);
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(parent.number() + 1))
        .output(
            CellOutput::new_builder()
                .capacity(capacity.pack())
                .lock(secp_script.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .witness(secp_script.clone().into_witness())
        .build()
}

pub fn gen_secp_block(
    blocks: &mut Vec<BlockView>,
    p_block: &BlockView,
    shared: &Shared,
) -> BlockView {
    let tx = create_secp_tx();
    let secp_cell_deps = vec![
        CellDep::new_builder()
            .out_point(OutPoint::new(tx.hash().unpack(), 0))
            .build(),
        CellDep::new_builder()
            .out_point(OutPoint::new(tx.hash().unpack(), 1))
            .build(),
    ];
    let (_, _, secp_script) = secp_cell();
    let (number, timestamp, difficulty) = (
        p_block.header().number() + 1,
        p_block.header().timestamp() + 10000,
        p_block.header().difficulty() + U256::from(1u64),
    );
    let cellbase = create_secp_cellbase(shared, &p_block.header());

    // spent n-2 block's tx and proposal n-1 block's tx
    let transactions: Vec<TransactionView> = if blocks.len() > 1 {
        let pp_block = shared
            .store()
            .get_block(&p_block.data().header().raw().parent_hash())
            .expect("gen_block get pp_block");
        pp_block
            .transactions()
            .iter()
            .skip(1)
            .map(|tx| {
                create_2out_transaction(
                    tx.output_pts(),
                    secp_script.clone(),
                    secp_cell_deps.clone(),
                )
            })
            .collect()
    } else {
        vec![]
    };

    let proposals: Vec<ProposalShortId> = p_block
        .transactions()
        .iter()
        .skip(1)
        .map(|tx| {
            create_2out_transaction(tx.output_pts(), secp_script.clone(), secp_cell_deps.clone())
                .proposal_short_id()
        })
        .collect();

    let mut txs_to_resolve = vec![cellbase.clone()];
    txs_to_resolve.extend_from_slice(&transactions);
    let dao = dao_data(shared, &p_block.header(), &txs_to_resolve);

    let block = BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .proposals(proposals)
        .parent_hash(p_block.hash())
        .number(number.pack())
        .timestamp(timestamp.pack())
        .difficulty(difficulty.pack())
        .nonce(random::<u64>().pack())
        .dao(dao)
        .build();

    blocks.push(block.clone());
    block
}

fn create_transaction(parent_hash: &H256, lock: Script, dep: OutPoint) -> TransactionView {
    let data: Bytes = (0..255).collect();
    TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(50_000).pack())
                .lock(lock.clone())
                .build(),
        )
        .output_data(data.pack())
        .input(CellInput::new(OutPoint::new(parent_hash.to_owned(), 0), 0))
        .cell_dep(CellDep::new_builder().out_point(dep).build())
        .build()
}

fn create_2out_transaction(
    inputs: Vec<OutPoint>,
    lock: Script,
    cell_deps: Vec<CellDep>,
) -> TransactionView {
    let data: Bytes = (0..255).collect();

    let cell_inputs = inputs.into_iter().map(|pts| CellInput::new(pts, 0));
    let cell_output = CellOutput::new_builder()
        .capacity(capacity_bytes!(50_000).pack())
        .lock(lock.clone())
        .build();

    let raw = TransactionBuilder::default()
        .output(cell_output.clone())
        .output(cell_output)
        .output_data(data.pack())
        .output_data(data.pack())
        .inputs(cell_inputs)
        .cell_deps(cell_deps)
        .build();

    let privkey: Privkey = PRIVKEY.into();

    let mut blake2b = ckb_hash::new_blake2b();
    let mut message = [0u8; 32];
    blake2b.update(&raw.hash().raw_data()[..]);
    blake2b.finalize(&mut message);
    let message = H256::from(message);
    let witness: Bytes = privkey
        .sign_recoverable(&message)
        .expect("sign tx")
        .serialize()
        .into();

    raw.as_advanced_builder()
        .witness(vec![witness.pack()].pack())
        .witness(vec![witness.pack()].pack())
        .build()
}

pub fn dao_data(shared: &Shared, parent: &HeaderView, txs: &[TransactionView]) -> Byte32 {
    let mut seen_inputs = HashSet::new();
    // In case of resolving errors, we just output a dummp DAO field,
    // since those should be the cases where we are testing invalid
    // blocks
    let transactions_provider = TransactionsProvider::new(txs.iter());
    let snapshot: &Snapshot = &shared.snapshot();
    let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, snapshot);
    let rtxs = txs.iter().try_fold(vec![], |mut rtxs, tx| {
        let rtx = resolve_transaction(tx, &mut seen_inputs, &overlay_cell_provider, snapshot);
        match rtx {
            Ok(rtx) => {
                rtxs.push(rtx);
                Ok(rtxs)
            }
            Err(e) => Err(e),
        }
    });
    let rtxs = rtxs.expect("dao_data resolve_transaction");
    let calculator = DaoCalculator::new(shared.consensus(), shared.store());
    calculator
        .dao_field(&rtxs, &parent)
        .expect("calculator dao_field")
}

pub(crate) fn calculate_reward(shared: &Shared, parent: &HeaderView) -> Capacity {
    let number = parent.number() + 1;
    let target_number = shared.consensus().finalize_target(number).unwrap();
    let target = shared
        .store()
        .get_ancestor(&parent.hash(), target_number)
        .expect("calculate_reward get_ancestor");
    let calculator = DaoCalculator::new(shared.consensus(), shared.store());
    calculator
        .primary_block_reward(&target)
        .expect("calculate_reward primary_block_reward")
        .safe_add(calculator.secondary_block_reward(&target).unwrap())
        .expect("calculate_reward safe_add")
}
