use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{resolve_transaction, OverlayCellProvider, TransactionsProvider};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{
    CellDep, CellInput, CellOutput, CellOutputBuilder, OutPoint, ProposalShortId, Transaction,
    TransactionBuilder,
};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_traits::chain_provider::ChainProvider;
use criterion::{criterion_group, criterion_main, Criterion};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use rand::random;
use std::collections::HashSet;
use std::sync::Arc;

#[cfg(not(feature = "ci"))]
const SIZES: &[usize] = &[100usize, 200, 500, 1000];

#[cfg(feature = "ci")]
const SIZES: &[usize] = &[10usize];

fn bench(c: &mut Criterion) {
    // benchmark processing 20 blocks on main branch
    c.bench_function_over_inputs(
        "main_branch",
        |b, txs_size| {
            b.iter_with_setup(
                || {
                    let (chains, out_point) = new_chain(**txs_size, 2);
                    let (ref chain1, ref shared1) = chains.0[0];
                    let (ref chain2, ref shared2) = chains.0[1];
                    let mut blocks =
                        vec![shared1.store().get_block(&shared1.genesis_hash()).unwrap()];
                    let mut parent = blocks[0].clone();
                    (0..20).for_each(|_| {
                        let block = gen_block(&mut blocks, &parent, shared2, &out_point);
                        chain2
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        parent = block;
                    });
                    (chain1.clone(), blocks)
                },
                |(chain, blocks)| {
                    blocks.into_iter().skip(1).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
                    });
                },
            )
        },
        SIZES,
    );

    // benchmark processing 2 blocks on side branch
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7
    c.bench_function_over_inputs(
        "side_branch",
        |b, txs_size| {
            b.iter_with_setup(
                || {
                    let (chains, out_point) = new_chain(**txs_size, 3);
                    let (ref chain1, ref shared1) = chains.0[0];
                    let (ref chain2, ref shared2) = chains.0[1];
                    let (ref chain3, ref shared3) = chains.0[2];
                    let mut blocks =
                        vec![shared1.store().get_block(&shared1.genesis_hash()).unwrap()];
                    let mut parent = blocks[0].clone();
                    (0..5).for_each(|i| {
                        let block = gen_block(&mut blocks, &parent, &shared2, &out_point);
                        chain2
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        if i < 2 {
                            chain3
                                .process_block(Arc::new(block.clone()), false)
                                .expect("process block OK");
                        }
                        parent = block;
                    });
                    let mut parent = blocks[2].clone();
                    (0..2).for_each(|_| {
                        let block = gen_block(&mut blocks, &parent, &shared3, &out_point);
                        chain3
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        parent = block;
                    });
                    blocks
                        .clone()
                        .into_iter()
                        .skip(1)
                        .take(5)
                        .for_each(|block| {
                            chain1
                                .process_block(Arc::new(block), true)
                                .expect("process block OK");
                        });
                    (chain1.clone(), blocks)
                },
                |(chain, blocks)| {
                    blocks.into_iter().skip(6).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
                    });
                },
            )
        },
        SIZES,
    );

    // benchmark processing 4 blocks for switching fork
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7 -> 8 -> 9
    c.bench_function_over_inputs(
        "switch_fork",
        |b, txs_size| {
            b.iter_with_setup(
                || {
                    let (chains, out_point) = new_chain(**txs_size, 3);
                    let (ref chain1, ref shared1) = chains.0[0];
                    let (ref chain2, ref shared2) = chains.0[1];
                    let (ref chain3, ref shared3) = chains.0[2];
                    let mut blocks =
                        vec![shared1.store().get_block(&shared1.genesis_hash()).unwrap()];
                    let mut parent = blocks[0].clone();
                    (0..5).for_each(|i| {
                        let block = gen_block(&mut blocks, &parent, &shared2, &out_point);
                        let arc_block = Arc::new(block.clone());
                        chain2
                            .process_block(Arc::clone(&arc_block), false)
                            .expect("process block OK");
                        if i < 2 {
                            chain3
                                .process_block(arc_block, false)
                                .expect("process block OK");
                        }
                        parent = block;
                    });
                    let mut parent = blocks[2].clone();
                    (0..4).for_each(|_| {
                        let block = gen_block(&mut blocks, &parent, &shared3, &out_point);
                        chain3
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        parent = block;
                    });
                    blocks
                        .clone()
                        .into_iter()
                        .skip(1)
                        .take(7)
                        .for_each(|block| {
                            chain1
                                .process_block(Arc::new(block), true)
                                .expect("process block OK");
                        });
                    (chain1.clone(), blocks)
                },
                |(chain, blocks)| {
                    blocks.into_iter().skip(8).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
                    });
                },
            )
        },
        SIZES,
    );
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench
);
criterion_main!(benches);

pub(crate) fn create_always_success_tx() -> Transaction {
    let (ref always_success_cell, ref always_success_cell_data, ref script) = always_success_cell();
    TransactionBuilder::default()
        .witness(script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.clone())
        .build()
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

pub(crate) fn create_cellbase(shared: &Shared, parent: &Header) -> Transaction {
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

#[derive(Default)]
pub struct Chains(pub Vec<(ChainController, Shared)>);

impl Chains {
    pub fn push(&mut self, chain: (ChainController, Shared)) {
        self.0.push(chain);
    }
}

fn new_chain(txs_size: usize, chains_num: usize) -> (Chains, OutPoint) {
    let (_, _, always_success_script) = always_success_cell();
    let tx = create_always_success_tx();
    let always_success_out_point = OutPoint::new(tx.hash().to_owned(), 0);
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

    (chains, always_success_out_point)
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

fn gen_block(
    blocks: &mut Vec<Block>,
    p_block: &Block,
    shared: &Shared,
    always_success_out_point: &OutPoint,
) -> Block {
    let (number, timestamp, difficulty) = (
        p_block.header().number() + 1,
        p_block.header().timestamp() + 10000,
        p_block.header().difficulty() + U256::from(1u64),
    );
    let cellbase = create_cellbase(shared, p_block.header());

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
            .map(|tx| create_transaction(tx.hash(), always_success_out_point.clone()))
            .collect()
    } else {
        vec![]
    };

    let proposals: Vec<ProposalShortId> = p_block
        .transactions()
        .iter()
        .skip(1)
        .map(|tx| {
            create_transaction(tx.hash(), always_success_out_point.clone()).proposal_short_id()
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

fn create_transaction(parent_hash: &H256, always_success_out_point: OutPoint) -> Transaction {
    let (_, _, always_success_script) = always_success_cell();
    let data: Bytes = (0..255).collect();
    TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(50_000),
            CellOutput::calculate_data_hash(&data),
            always_success_script.clone(),
            None,
        ))
        .output_data(data)
        .input(CellInput::new(OutPoint::new(parent_hash.to_owned(), 0), 0))
        .dep(CellDep::Cell(always_success_out_point))
        .build()
}
