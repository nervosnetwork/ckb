use crate::benchmarks::util::{create_2out_transaction, create_secp_tx, secp_cell};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_dao_utils::genesis_dao_data;
use ckb_jsonrpc_types::JsonBytes;
use ckb_notify::NotifyService;
use ckb_shared::{
    shared::{Shared, SharedBuilder},
    Snapshot,
};
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_tx_pool::BlockAssemblerConfig;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, BlockView, Capacity, ScriptHashType, TransactionBuilder,
        TransactionView,
    },
    packed::{Block, CellDep, CellInput, CellOutput, Header, OutPoint},
    prelude::*,
    U256,
};
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use criterion::{criterion_group, Criterion};
use futures::future::Future;
use rand::random;
use std::sync::Arc;

#[cfg(not(feature = "ci"))]
const SIZES: &[usize] = &[500];

#[cfg(feature = "ci")]
const SIZES: &[usize] = &[2usize];

fn block_assembler_config() -> BlockAssemblerConfig {
    let (_, _, secp_script) = secp_cell();
    let args = secp_script
        .args()
        .into_iter()
        .map(|bytes| JsonBytes::from_bytes(bytes.unpack()))
        .collect();
    let hash_type: ScriptHashType = secp_script.hash_type().unpack();

    BlockAssemblerConfig {
        code_hash: secp_script.code_hash().unpack(),
        hash_type: hash_type.into(),
        args,
    }
}

pub fn setup_chain(txs_size: usize) -> (Shared, ChainController) {
    let (_, _, secp_script) = secp_cell();
    let tx = create_secp_tx();
    let dao = genesis_dao_data(vec![&tx]).unwrap();

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

    let (shared, table) = SharedBuilder::default()
        .consensus(consensus.clone())
        .block_assembler_config(Some(block_assembler_config()))
        .build()
        .unwrap();
    let chain_service = ChainService::new(shared.clone(), table);
    let chain_controller = chain_service.start(Some("ChainService"));

    (shared, chain_controller)
}

pub fn gen_txs_from_block(block: &BlockView) -> Vec<TransactionView> {
    let tx = create_secp_tx();
    let secp_cell_deps = vec![
        CellDep::new_builder()
            .out_point(OutPoint::new(tx.hash(), 0))
            .build(),
        CellDep::new_builder()
            .out_point(OutPoint::new(tx.hash(), 1))
            .build(),
    ];
    let (_, _, secp_script) = secp_cell();
    // spent n-2 block's tx and proposal n-1 block's tx
    if block.transactions().len() > 1 {
        block
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
    }
}

fn bench(c: &mut Criterion) {
    c.bench_function_over_inputs(
        "overall",
        |b, txs_size| {
            b.iter_with_setup(
                || setup_chain(**txs_size),
                |(shared, chain)| {
                    let mut i = 10;
                    while i > 0 {
                        let snapshot: &Snapshot = &shared.snapshot();
                        let tip_hash = snapshot.tip_hash();
                        let block = snapshot.get_block(&tip_hash).expect("tip exist");
                        let txs = gen_txs_from_block(&block);

                        let tx_pool = shared.tx_pool_controller();
                        if !txs.is_empty() {
                            tx_pool.submit_txs(txs).unwrap().wait().unwrap();
                        }
                        let block_template = tx_pool
                            .get_block_template(None, None, None)
                            .unwrap()
                            .wait()
                            .unwrap()
                            .unwrap();
                        let raw_block: Block = block_template.into();
                        let raw_header = raw_block.header().raw();
                        let header = Header::new_builder()
                            .raw(raw_header)
                            .nonce(random::<u64>().pack())
                            .build();
                        let block = raw_block.as_builder().header(header).build().into_view();

                        let header_view = block.header();
                        let resolver = HeaderResolverWrapper::new(
                            &header_view,
                            shared.store(),
                            shared.consensus(),
                        );
                        let header_verifier = HeaderVerifier::new(
                            snapshot,
                            Arc::clone(&shared.consensus().pow_engine()),
                        );
                        header_verifier.verify(&resolver).expect("header verified");

                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process_block");
                        i -= 1;
                    }
                },
            )
        },
        SIZES,
    );
}

criterion_group!(
    name = overall;
    config = Criterion::default().sample_size(10);
    targets = bench
);
