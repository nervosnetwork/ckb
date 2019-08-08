use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{resolve_transaction, OverlayCellProvider, TransactionsProvider};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::script::{Script, ScriptHashType};
use ckb_core::transaction::{
    CellDep, CellInput, CellOutput, CellOutputBuilder, OutPoint, ProposalShortId, Transaction,
    TransactionBuilder,
};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_crypto::secp::Privkey;
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::always_success_cell;
use ckb_traits::chain_provider::ChainProvider;
use lazy_static::lazy_static;
use numext_fixed_hash::{h160, h256, H160, H256};
use numext_fixed_uint::U256;
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
    let header_builder = HeaderBuilder::default()
        .dao(dao)
        .difficulty(U256::from(1000u64));

    // create genesis block with N txs
    let transactions: Vec<Transaction> = (0..txs_size)
        .map(|i| {
            let data = Bytes::from(i.to_le_bytes().to_vec());
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutputBuilder::from_data(&data)
                        .capacity(capacity_bytes!(50_000))
                        .lock(always_success_script.clone())
                        .build(),
                )
                .output_data(data)
                .build()
        })
        .collect();

    let genesis_block = BlockBuilder::from_header_builder(header_builder)
        .transaction(tx)
        .transactions(transactions)
        .build();

    let mut consensus = Consensus::default()
        .set_cellbase_maturity(0)
        .set_genesis_block(genesis_block);
    consensus.tx_proposal_window = ProposalWindow(1, 10);

    let mut chains = Chains::default();

    for _ in 0..chains_num {
        let shared = SharedBuilder::default()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let notify = NotifyService::default().start::<&str>(None);
        let chain_service = ChainService::new(shared.clone(), notify);

        chains.push((chain_service.start::<&str>(None), shared));
    }

    chains
}

pub fn create_always_success_tx() -> Transaction {
    let (ref always_success_cell, ref always_success_cell_data, ref script) = always_success_cell();
    TransactionBuilder::default()
        .witness(script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.clone())
        .build()
}

pub fn create_always_success_cellbase(shared: &Shared, parent: &Header) -> Transaction {
    let (_, _, always_success_script) = always_success_cell();
    let capacity = calculate_reward(shared, parent);
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(parent.number() + 1))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity)
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(Bytes::new())
        .witness(always_success_script.clone().into_witness())
        .build()
}

pub fn gen_always_success_block(
    blocks: &mut Vec<Block>,
    p_block: &Block,
    shared: &Shared,
) -> Block {
    let tx = create_always_success_tx();
    let always_success_out_point = OutPoint::new(tx.hash().to_owned(), 0);
    let (_, _, always_success_script) = always_success_cell();
    let (number, timestamp, difficulty) = (
        p_block.header().number() + 1,
        p_block.header().timestamp() + 10000,
        p_block.header().difficulty() + U256::from(1u64),
    );
    let cellbase = create_always_success_cellbase(shared, p_block.header());

    // spent n-2 block's tx and proposal n-1 block's tx
    let transactions: Vec<Transaction> = if blocks.len() > 1 {
        let pp_block = shared
            .store()
            .get_block(p_block.header().parent_hash())
            .expect("gen_block get pp_block");
        pp_block
            .transactions()
            .iter()
            .skip(1)
            .map(|tx| {
                create_transaction(
                    tx.hash(),
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
                tx.hash(),
                always_success_script.clone(),
                always_success_out_point.clone(),
            )
            .proposal_short_id()
        })
        .collect();

    let mut txs_to_resolve = vec![cellbase.clone()];
    txs_to_resolve.extend_from_slice(&transactions);
    let dao = dao_data(shared, p_block.header(), &txs_to_resolve);

    let block = BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .proposals(proposals)
        .header_builder(
            HeaderBuilder::default()
                .parent_hash(p_block.header().hash().to_owned())
                .number(number)
                .timestamp(timestamp)
                .difficulty(difficulty)
                .nonce(random())
                .dao(dao),
        )
        .build();

    blocks.push(block.clone());
    block
}

const PRIVKEY: H256 = h256!("0xb2b3324cece882bca684eaf202667bb56ed8e8c2fd4b4dc71f615ebd6d9055a5");
const PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");

lazy_static! {
    static ref SECP_CELL: (CellOutput, Bytes, Script) = {
        let raw_data = BUNDLED_CELL
            .get("specs/cells/secp256k1_blake160_sighash_all")
            .expect("load secp256k1_blake160_sighash_all");
        let data: Bytes = raw_data[..].into();

        let cell = CellOutput::new(
            Capacity::bytes(data.len()).unwrap(),
            CellOutput::calculate_data_hash(&data),
            Script::default(),
            None,
        );

        let script = Script::new(
            vec![Bytes::from(PUBKEY_HASH.as_bytes())],
            cell.data_hash().to_owned(),
            ScriptHashType::Data,
        );

        (cell, data, script)
    };
}

pub fn secp_cell() -> &'static (CellOutput, Bytes, Script) {
    &SECP_CELL
}

pub fn create_secp_tx() -> Transaction {
    let (ref cell, ref cell_data, ref script) = secp_cell();
    TransactionBuilder::default()
        .witness(script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(cell.clone())
        .output_data(cell_data.clone())
        .build()
}

pub fn new_secp_chain(txs_size: usize, chains_num: usize) -> Chains {
    let (_, _, secp_script) = secp_cell();
    let tx = create_secp_tx();
    let dao = genesis_dao_data(&tx).unwrap();
    let header_builder = HeaderBuilder::default()
        .dao(dao)
        .difficulty(U256::from(1000u64));

    // create genesis block with N txs
    let transactions: Vec<Transaction> = (0..txs_size)
        .map(|i| {
            let data = Bytes::from(i.to_le_bytes().to_vec());
            let output = CellOutputBuilder::from_data(&data)
                .capacity(capacity_bytes!(50_000))
                .lock(secp_script.clone())
                .build();
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(output.clone())
                .output(output)
                .output_data(data.clone())
                .output_data(data)
                .build()
        })
        .collect();

    let genesis_block = BlockBuilder::from_header_builder(header_builder)
        .transaction(tx)
        .transactions(transactions)
        .build();

    let mut consensus = Consensus::default()
        .set_cellbase_maturity(0)
        .set_genesis_block(genesis_block);
    consensus.tx_proposal_window = ProposalWindow(1, 10);

    let mut chains = Chains::default();

    for _ in 0..chains_num {
        let shared = SharedBuilder::default()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let notify = NotifyService::default().start::<&str>(None);
        let chain_service = ChainService::new(shared.clone(), notify);

        chains.push((chain_service.start::<&str>(None), shared));
    }

    chains
}

pub fn create_secp_cellbase(shared: &Shared, parent: &Header) -> Transaction {
    let (_, _, secp_script) = secp_cell();
    let capacity = calculate_reward(shared, parent);
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(parent.number() + 1))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity)
                .lock(secp_script.clone())
                .build(),
        )
        .output_data(Bytes::new())
        .witness(secp_script.clone().into_witness())
        .build()
}

pub fn gen_secp_block(blocks: &mut Vec<Block>, p_block: &Block, shared: &Shared) -> Block {
    let tx = create_secp_tx();
    let secp_out_point = OutPoint::new(tx.hash().to_owned(), 0);
    let (_, _, secp_script) = secp_cell();
    let (number, timestamp, difficulty) = (
        p_block.header().number() + 1,
        p_block.header().timestamp() + 10000,
        p_block.header().difficulty() + U256::from(1u64),
    );
    let cellbase = create_secp_cellbase(shared, p_block.header());

    // spent n-2 block's tx and proposal n-1 block's tx
    let transactions: Vec<Transaction> = if blocks.len() > 1 {
        let pp_block = shared
            .store()
            .get_block(p_block.header().parent_hash())
            .expect("gen_block get pp_block");
        pp_block
            .transactions()
            .iter()
            .skip(1)
            .map(|tx| {
                create_2out_transaction(
                    tx.output_pts(),
                    secp_script.clone(),
                    secp_out_point.clone(),
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
            create_2out_transaction(tx.output_pts(), secp_script.clone(), secp_out_point.clone())
                .proposal_short_id()
        })
        .collect();

    let mut txs_to_resolve = vec![cellbase.clone()];
    txs_to_resolve.extend_from_slice(&transactions);
    let dao = dao_data(shared, p_block.header(), &txs_to_resolve);

    let block = BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .proposals(proposals)
        .header_builder(
            HeaderBuilder::default()
                .parent_hash(p_block.header().hash().to_owned())
                .number(number)
                .timestamp(timestamp)
                .difficulty(difficulty)
                .nonce(random())
                .dao(dao),
        )
        .build();

    blocks.push(block.clone());
    block
}

fn create_transaction(parent_hash: &H256, lock: Script, dep: OutPoint) -> Transaction {
    let data: Bytes = (0..255).collect();
    TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(50_000),
            CellOutput::calculate_data_hash(&data),
            lock.clone(),
            None,
        ))
        .output_data(data)
        .input(CellInput::new(OutPoint::new(parent_hash.to_owned(), 0), 0))
        .cell_dep(CellDep::new_cell(dep))
        .build()
}

fn create_2out_transaction(inputs: Vec<OutPoint>, lock: Script, dep: OutPoint) -> Transaction {
    let data: Bytes = (0..255).collect();

    let cell_inputs = inputs.into_iter().map(|pts| CellInput::new(pts, 0));
    let cell_output = CellOutput::new(
        capacity_bytes!(50_000),
        CellOutput::calculate_data_hash(&data),
        lock.clone(),
        None,
    );

    let raw = TransactionBuilder::default()
        .output(cell_output.clone())
        .output(cell_output)
        .output_data(data.clone())
        .output_data(data)
        .inputs(cell_inputs)
        .cell_dep(CellDep::new_cell(dep))
        .build();

    let privkey: Privkey = PRIVKEY.into();

    let mut blake2b = ckb_hash::new_blake2b();
    let mut message = [0u8; 32];
    blake2b.update(&raw.hash()[..]);
    blake2b.finalize(&mut message);
    let message = H256::from(message);
    let witness: Bytes = privkey
        .sign_recoverable(&message)
        .expect("sign tx")
        .serialize()
        .into();

    TransactionBuilder::from_transaction(raw)
        .witness(vec![witness.clone()])
        .witness(vec![witness])
        .build()
}

pub fn dao_data(shared: &Shared, parent: &Header, txs: &[Transaction]) -> Bytes {
    let mut seen_inputs = HashSet::default();
    // In case of resolving errors, we just output a dummp DAO field,
    // since those should be the cases where we are testing invalid
    // blocks
    let transactions_provider = TransactionsProvider::new(txs);
    let chain_state = shared.lock_chain_state();
    let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, &*chain_state);
    let rtxs = txs.iter().try_fold(vec![], |mut rtxs, tx| {
        let rtx = resolve_transaction(tx, &mut seen_inputs, &overlay_cell_provider, &*chain_state);
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

pub(crate) fn calculate_reward(shared: &Shared, parent: &Header) -> Capacity {
    let number = parent.number() + 1;
    let target_number = shared.consensus().finalize_target(number).unwrap();
    let target = shared
        .store()
        .get_ancestor(parent.hash(), target_number)
        .expect("calculate_reward get_ancestor");
    let calculator = DaoCalculator::new(shared.consensus(), shared.store());
    calculator
        .primary_block_reward(&target)
        .expect("calculate_reward primary_block_reward")
        .safe_add(calculator.secondary_block_reward(&target).unwrap())
        .expect("calculate_reward safe_add")
}
