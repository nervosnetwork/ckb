use crate::benchmarks::util::create_2out_transaction;
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::{ChainSpec, IssuedCell};
use ckb_jsonrpc_types::JsonBytes;
use ckb_launcher::SharedBuilder;
use ckb_resource::Resource;
use ckb_shared::{Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{resolve_transaction, setup_system_cell_cache},
        BlockView, Capacity, DepType, FeeRate, ScriptHashType, TransactionView,
    },
    h160, h256,
    packed::{CellDep, OutPoint, Script},
    prelude::*,
    H160, H256,
};
use criterion::{criterion_group, BatchSize, BenchmarkId, Criterion};
use std::collections::HashSet;
use std::convert::TryFrom;

#[cfg(not(feature = "ci"))]
const SIZE: usize = 500;

#[cfg(feature = "ci")]
const SIZE: usize = 10;

const PUBKEY_HASH: H160 = h160!("0x779e5930892a0a9bf2fedfe048f685466c7d0396");
const DEFAULT_CODE_HASH: H256 =
    h256!("0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8");

fn script() -> Script {
    Script::new_builder()
        .code_hash(DEFAULT_CODE_HASH.pack())
        .args(Bytes::from(PUBKEY_HASH.as_bytes()).pack())
        .hash_type(ScriptHashType::Type.into())
        .build()
}

fn cell_dep(genesis: &BlockView) -> CellDep {
    let tx_hash = genesis.transaction(1).unwrap().hash();
    let out_point = OutPoint::new_builder()
        .tx_hash(tx_hash)
        .index(0u32.pack())
        .build();

    CellDep::new_builder()
        .out_point(out_point)
        .dep_type(DepType::DepGroup.into())
        .build()
}

fn block_assembler_config() -> BlockAssemblerConfig {
    let secp_script = script();
    let args = JsonBytes::from_bytes(secp_script.args().unpack());
    let hash_type = ScriptHashType::try_from(secp_script.hash_type()).expect("checked data");

    BlockAssemblerConfig {
        code_hash: secp_script.code_hash().unpack(),
        hash_type: hash_type.into(),
        args,
        message: Default::default(),
    }
}

pub fn setup_chain(txs_size: usize) -> (Shared, ChainController) {
    let secp_script = script();

    let mut spec =
        ChainSpec::load_from(&Resource::bundled("specs/mainnet.toml".to_string())).unwrap();
    spec.genesis.issued_cells = (0..txs_size)
        .map(|_| IssuedCell {
            capacity: capacity_bytes!(100_000),
            lock: secp_script.clone().into(),
        })
        .collect();

    let consensus = spec.build_consensus().unwrap();

    let tx_pool_config = TxPoolConfig {
        min_fee_rate: FeeRate::from_u64(0),
        ..Default::default()
    };

    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(consensus)
        .block_assembler_config(Some(block_assembler_config()))
        .tx_pool_config(tx_pool_config)
        .build()
        .unwrap();
    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let chain_controller = chain_service.start(Some("ChainService"));

    // FIXME: global cache !!!
    let _ret = setup_system_cell_cache(
        shared.consensus().genesis_block(),
        &shared.store().cell_provider(),
    );

    (shared, chain_controller)
}

pub fn gen_txs_from_genesis(block: &BlockView) -> Vec<TransactionView> {
    let cell_deps = vec![cell_dep(block)];
    let script = script();
    // spent n-2 block's tx and proposal n-1 block's tx

    let outputs = block.transaction(0).unwrap().output_pts();
    outputs
        .into_iter()
        .skip(6)
        .map(|pt| create_2out_transaction(vec![pt], script.clone(), cell_deps.clone()))
        .collect()
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolve");

    group.bench_with_input(BenchmarkId::new("resolve", SIZE), &SIZE, |b, txs_size| {
        b.iter_batched(
            || setup_chain(*txs_size),
            |(shared, _)| {
                let mut i = 100;
                let snapshot: &Snapshot = &shared.snapshot();
                let txs = gen_txs_from_genesis(&shared.consensus().genesis_block());

                let provider = snapshot.cell_provider();
                while i > 0 {
                    let mut seen_inputs = HashSet::new();

                    for tx in txs.clone() {
                        resolve_transaction(tx, &mut seen_inputs, &provider, snapshot).unwrap();
                    }

                    i -= 1;
                }
            },
            BatchSize::PerIteration,
        )
    });

    group.bench_with_input(
        BenchmarkId::new("check_resolve", SIZE),
        &SIZE,
        |b, txs_size| {
            b.iter_batched(
                || setup_chain(*txs_size),
                |(shared, _)| {
                    let mut i = 1;
                    let snapshot: &Snapshot = &shared.snapshot();
                    let txs = gen_txs_from_genesis(&shared.consensus().genesis_block());

                    let provider = snapshot.cell_provider();
                    let mut seen_inputs = HashSet::new();
                    let rtxs: Vec<_> = txs
                        .into_iter()
                        .map(|tx| {
                            resolve_transaction(tx, &mut seen_inputs, &provider, snapshot).unwrap()
                        })
                        .collect();

                    while i > 0 {
                        let mut seen_inputs = HashSet::new();
                        for rtx in &rtxs {
                            rtx.check(&mut seen_inputs, &provider, snapshot).unwrap();
                        }
                        i -= 1;
                    }
                },
                BatchSize::PerIteration,
            )
        },
    );
}

criterion_group!(
    name = resolve;
    config = Criterion::default().sample_size(10);
    targets = bench
);
