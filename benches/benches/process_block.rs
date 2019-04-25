use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::{capacity_bytes, Capacity};
use ckb_db::{CacheDB, DBConfig, RocksDB};
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_shared::store::ChainKVStore;
use ckb_traits::ChainProvider;
use criterion::{criterion_group, criterion_main, Criterion};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use rand::random;
use std::sync::Arc;
use tempfile::{tempdir, TempDir};

fn bench(c: &mut Criterion) {
    let txs_sizes = vec![100usize, 200, 500, 1000];

    // benchmark processing 20 blocks on main branch
    c.bench_function_over_inputs(
        "main_branch",
        |b, txs_size| {
            b.iter_with_setup(
                || {
                    let (chain, shared, dir, system_cell_hash, data_hash) = new_chain(*txs_size);
                    let mut blocks = vec![shared.block(&shared.genesis_hash()).unwrap()];
                    (0..20).for_each(|_| {
                        let parent_index = blocks.len() - 1;
                        gen_block(&mut blocks, parent_index, &system_cell_hash, &data_hash);
                    });
                    (chain, blocks, dir)
                },
                |(chain, blocks, _dir)| {
                    blocks.into_iter().skip(1).for_each(|block| {
                        chain
                            .process_block(Arc::new(block))
                            .expect("process block OK")
                    });
                },
            )
        },
        txs_sizes.clone(),
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
                    let (chain, shared, dir, system_cell_hash, data_hash) = new_chain(*txs_size);
                    let mut blocks = vec![shared.block(&shared.genesis_hash()).unwrap()];
                    (0..5).for_each(|_| {
                        let parent_index = blocks.len() - 1;
                        gen_block(&mut blocks, parent_index, &system_cell_hash, &data_hash);
                    });
                    (0..2).for_each(|i| {
                        let parent_index = i + 2;
                        gen_block(&mut blocks, parent_index, &system_cell_hash, &data_hash);
                    });
                    blocks
                        .clone()
                        .into_iter()
                        .skip(1)
                        .take(5)
                        .for_each(|block| {
                            chain
                                .process_block(Arc::new(block))
                                .expect("process block OK")
                        });
                    (chain, blocks, dir)
                },
                |(chain, blocks, _dir)| {
                    blocks.into_iter().skip(6).for_each(|block| {
                        chain
                            .process_block(Arc::new(block))
                            .expect("process block OK")
                    });
                },
            )
        },
        txs_sizes.clone(),
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
                    let (chain, shared, dir, system_cell_hash, data_hash) = new_chain(*txs_size);
                    let mut blocks = vec![shared.block(&shared.genesis_hash()).unwrap()];
                    (0..5).for_each(|_| {
                        let parent_index = blocks.len() - 1;
                        gen_block(&mut blocks, parent_index, &system_cell_hash, &data_hash);
                    });
                    (0..4).for_each(|i| {
                        let parent_index = i + 2;
                        gen_block(&mut blocks, parent_index, &system_cell_hash, &data_hash);
                    });
                    blocks
                        .clone()
                        .into_iter()
                        .skip(1)
                        .take(7)
                        .for_each(|block| {
                            chain
                                .process_block(Arc::new(block))
                                .expect("process block OK")
                        });
                    (chain, blocks, dir)
                },
                |(chain, blocks, _dir)| {
                    blocks.into_iter().skip(8).for_each(|block| {
                        chain
                            .process_block(Arc::new(block))
                            .expect("process block OK")
                    });
                },
            )
        },
        txs_sizes.clone(),
    );
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench
);
criterion_main!(benches);

fn new_chain(
    txs_size: usize,
) -> (
    ChainController,
    Shared<ChainKVStore<CacheDB<RocksDB>>>,
    TempDir,
    H256,
    H256,
) {
    let always_success = include_bytes!("../../resource/specs/cells/always_success");
    let cell_output = CellOutput::new(
        Capacity::bytes(always_success.len()).unwrap(),
        always_success.to_vec(),
        Script::default(),
        None,
    );
    let data_hash = cell_output.data_hash();

    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(cell_output)
        .build();

    let system_cell_hash = cellbase.hash();

    // create genesis block with N txs
    let transactions: Vec<Transaction> = (0..txs_size)
        .map(|i| {
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0, vec![]))
                .output(CellOutput::new(
                    capacity_bytes!(50_000),
                    i.to_le_bytes().to_vec(),
                    Script::new(Vec::new(), data_hash.clone()),
                    None,
                ))
                .build()
        })
        .collect();

    let genesis_block = BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.tx_proposal_window = ProposalWindow(1, 10);
    consensus.cellbase_maturity = 0;

    let db_dir = tempdir().unwrap();
    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .db(&DBConfig {
            path: db_dir.path().to_owned(),
            options: None,
        })
        .consensus(consensus)
        .build();
    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainBuilder::new(shared.clone(), notify).build();
    (
        chain_service.start::<&str>(None),
        shared,
        db_dir,
        system_cell_hash,
        data_hash,
    )
}

fn gen_block(
    blocks: &mut Vec<Block>,
    parent_index: usize,
    system_cell_hash: &H256,
    data_hash: &H256,
) {
    let p_block = &blocks[parent_index];

    let (number, timestamp, difficulty) = (
        p_block.header().number() + 1,
        p_block.header().timestamp() + 10000,
        p_block.header().difficulty() + U256::from(1u64),
    );

    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(
            Capacity::zero(),
            vec![],
            Script::default(),
            None,
        ))
        .build();

    // spent n-2 block's tx and proposal n-1 block's tx
    let transactions: Vec<Transaction> = if blocks.len() > 1 {
        let pp_block = &blocks[parent_index - 1];
        pp_block
            .transactions()
            .iter()
            .skip(1)
            .map(|tx| create_transaction(tx.hash(), system_cell_hash, data_hash))
            .collect()
    } else {
        vec![]
    };

    let proposals: Vec<ProposalShortId> = p_block
        .transactions()
        .iter()
        .skip(1)
        .map(|tx| create_transaction(tx.hash(), system_cell_hash, data_hash).proposal_short_id())
        .collect();

    let block = BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .proposals(proposals)
        .with_header_builder(
            HeaderBuilder::default()
                .parent_hash(p_block.header().hash().clone())
                .number(number)
                .timestamp(timestamp)
                .difficulty(difficulty)
                .nonce(random()),
        );

    blocks.push(block);
}

fn create_transaction(parent_hash: H256, system_cell_hash: &H256, data_hash: &H256) -> Transaction {
    TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(50_000),
            (0..255).collect(),
            Script::new(vec![(0..255).collect()], data_hash.clone()),
            None,
        ))
        .input(CellInput::new(OutPoint::new(parent_hash, 0), 0, vec![]))
        .dep(OutPoint::new(system_cell_hash.clone(), 0))
        .build()
}
