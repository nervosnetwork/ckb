use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::{Script, ScriptHashType};
use ckb_core::transaction::{
    CellInput, CellOutput, CellOutputBuilder, OutPoint, ProposalShortId, Transaction,
    TransactionBuilder,
};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_db::DBConfig;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
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
                    let mut blocks =
                        vec![shared.store().get_block(&shared.genesis_hash()).unwrap()];
                    (0..20).for_each(|_| {
                        let parent_index = blocks.len() - 1;
                        gen_block(&mut blocks, parent_index, &system_cell_hash, &data_hash);
                    });
                    (chain, blocks, dir)
                },
                |(chain, blocks, _dir)| {
                    blocks.into_iter().skip(1).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
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
                    let mut blocks =
                        vec![shared.store().get_block(&shared.genesis_hash()).unwrap()];
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
                                .process_block(Arc::new(block), true)
                                .expect("process block OK");
                        });
                    (chain, blocks, dir)
                },
                |(chain, blocks, _dir)| {
                    blocks.into_iter().skip(6).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
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
                    let mut blocks =
                        vec![shared.store().get_block(&shared.genesis_hash()).unwrap()];
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
                                .process_block(Arc::new(block), true)
                                .expect("process block OK");
                        });
                    (chain, blocks, dir)
                },
                |(chain, blocks, _dir)| {
                    blocks.into_iter().skip(8).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
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

fn new_chain(txs_size: usize) -> (ChainController, Shared, TempDir, H256, H256) {
    let always_success = include_bytes!("../../script/testdata/always_success");
    let data = Bytes::from(always_success.to_vec());
    let mut cell_output = CellOutputBuilder::from_data(&data).build();
    cell_output.capacity = cell_output
        .occupied_capacity(Capacity::bytes(data.len()).unwrap())
        .unwrap();

    let data_hash = cell_output.data_hash().to_owned();

    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(
            Script {
                args: vec![],
                code_hash: cell_output.data_hash().to_owned(),
                hash_type: ScriptHashType::Data,
            }
            .into_witness(),
        )
        .output(cell_output)
        .output_data(data)
        .build();

    let system_cell_hash = cellbase.hash().to_owned();

    // create genesis block with N txs
    let transactions: Vec<Transaction> = (0..txs_size)
        .map(|i| {
            let data = Bytes::from(i.to_le_bytes().to_vec());
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutputBuilder::from_data(&data)
                        .capacity(capacity_bytes!(50_000))
                        .lock(Script::new(
                            Vec::new(),
                            data_hash.clone(),
                            ScriptHashType::Data,
                        ))
                        .build(),
                )
                .output_data(data)
                .build()
        })
        .collect();

    let genesis_block = BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)))
        .build();

    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.tx_proposal_window = ProposalWindow(1, 10);
    consensus.cellbase_maturity = 0;

    let db_dir = tempdir().unwrap();
    let shared = SharedBuilder::with_db_config(&DBConfig {
        path: db_dir.path().to_owned(),
        options: None,
    })
    .consensus(consensus)
    .build()
    .unwrap();
    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainService::new(shared.clone(), notify);
    (
        chain_service.start::<&str>(None),
        shared,
        db_dir,
        system_cell_hash,
        data_hash.to_owned(),
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

    let mut cell_output = CellOutput::default();
    cell_output.capacity = cell_output.occupied_capacity(Capacity::zero()).unwrap();

    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(cell_output)
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
        .header_builder(
            HeaderBuilder::default()
                .parent_hash(p_block.header().hash().to_owned())
                .number(number)
                .timestamp(timestamp)
                .difficulty(difficulty)
                .nonce(random()),
        )
        .build();

    blocks.push(block);
}

fn create_transaction(
    parent_hash: &H256,
    system_cell_hash: &H256,
    data_hash: &H256,
) -> Transaction {
    let data: Bytes = (0..255).collect();
    TransactionBuilder::default()
        .output(
            CellOutputBuilder::from_data(&data)
                .capacity(capacity_bytes!(50_000))
                .lock(Script::new(
                    vec![(0..255).collect()],
                    data_hash.to_owned(),
                    ScriptHashType::Data,
                ))
                .build(),
        )
        .output_data(data)
        .input(CellInput::new(
            OutPoint::new_cell(parent_hash.to_owned(), 0),
            0,
        ))
        .dep(OutPoint::new_cell(system_cell_hash.to_owned(), 0))
        .build()
}
