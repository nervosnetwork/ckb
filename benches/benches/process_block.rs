use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
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
    // benchmark processing 5 blocks on main branch
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    c.bench_function("main_branch", |b| {
        b.iter_with_setup(
            || {
                let (chain, shared, dir) = new_chain();
                let mut blocks = vec![shared.block(&shared.genesis_hash()).unwrap()];
                (0..5).for_each(|_| {
                    let parent_index = blocks.len() - 1;
                    gen_block(&mut blocks, parent_index);
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
    });

    // benchmark processing 2 blocks on side branch
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7
    c.bench_function("side_branch", |b| {
        b.iter_with_setup(
            || {
                let (chain, shared, dir) = new_chain();
                let mut blocks = vec![shared.block(&shared.genesis_hash()).unwrap()];
                (0..5).for_each(|_| {
                    let parent_index = blocks.len() - 1;
                    gen_block(&mut blocks, parent_index);
                });
                (0..2).for_each(|i| {
                    let parent_index = i + 2;
                    gen_block(&mut blocks, parent_index);
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
    });

    // benchmark processing 4 blocks for switching fork
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7 -> 8 -> 9
    c.bench_function("switch_fork", |b| {
        b.iter_with_setup(
            || {
                let (chain, shared, dir) = new_chain();
                let mut blocks = vec![shared.block(&shared.genesis_hash()).unwrap()];
                (0..5).for_each(|_| {
                    let parent_index = blocks.len() - 1;
                    gen_block(&mut blocks, parent_index);
                });
                (0..4).for_each(|i| {
                    let parent_index = i + 2;
                    gen_block(&mut blocks, parent_index);
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
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = bench
);
criterion_main!(benches);

fn new_chain() -> (
    ChainController,
    Shared<ChainKVStore<CacheDB<RocksDB>>>,
    TempDir,
) {
    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0, 0))
        .output(CellOutput::new(0, vec![], Script::default(), None))
        .build();

    // create genesis block with 100 tx
    let commit_transactions: Vec<Transaction> = (0..100)
        .map(|i| {
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0, vec![]))
                .output(CellOutput::new(
                    50000,
                    vec![i],
                    Script::always_success(),
                    None,
                ))
                .build()
        })
        .collect();

    let genesis_block = BlockBuilder::default()
        .commit_transaction(cellbase)
        .commit_transactions(commit_transactions)
        .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

    let consensus = Consensus::default().set_genesis_block(genesis_block);

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
    (chain_service.start::<&str>(None), shared, db_dir)
}

fn gen_block(blocks: &mut Vec<Block>, parent_index: usize) {
    let p_block = &blocks[parent_index];

    let (number, timestamp, difficulty) = (
        p_block.header().number() + 1,
        p_block.header().timestamp() + 10000,
        p_block.header().difficulty() + U256::from(1u64),
    );

    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number, 0))
        .output(CellOutput::new(0, vec![], Script::default(), None))
        .build();

    // spent n-2 block's tx and proposal n-1 block's tx
    let commit_transactions: Vec<Transaction> = if blocks.len() > parent_index + 1 {
        let pp_block = &blocks[parent_index - 1];
        pp_block
            .commit_transactions()
            .iter()
            .skip(1)
            .map(|tx| create_transaction(tx.hash()))
            .collect()
    } else {
        vec![]
    };

    let proposal_transactions: Vec<ProposalShortId> = p_block
        .commit_transactions()
        .iter()
        .skip(1)
        .map(|tx| create_transaction(tx.hash()).proposal_short_id())
        .collect();

    let block = BlockBuilder::default()
        .commit_transaction(cellbase)
        .commit_transactions(commit_transactions)
        .proposal_transactions(proposal_transactions)
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

fn create_transaction(hash: H256) -> Transaction {
    TransactionBuilder::default()
        .output(CellOutput::new(
            50000,
            vec![],
            Script::always_success(),
            None,
        ))
        .input(CellInput::new(OutPoint::new(hash, 0), 0, vec![]))
        .build()
}
