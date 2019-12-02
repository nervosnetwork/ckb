use ckb_chain_spec::consensus::{build_genesis_epoch_ext, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_types::{
    core::{
        capacity_bytes, BlockBuilder, BlockView, Capacity, HeaderBuilder, HeaderContextType,
        HeaderView, TransactionBuilder,
    },
    packed::{Byte32, CellInput, Script},
    prelude::*,
    utilities::DIFF_TWO,
};
use criterion::{criterion_group, Criterion};
use faketime::unix_time_as_millis;
use rand::{thread_rng, Rng};
use std::collections::HashMap;

const GENESIS_TARGET: u32 = 0x2001_0000;
const DEFAULT_EPOCH_REWARD: Capacity = capacity_bytes!(1_250_000);
const MIN_BLOCK_INTERVAL: u64 = 8;

#[cfg(not(feature = "ci"))]
const SAMPLES: &[usize] = &[100usize, 500];

#[cfg(feature = "ci")]
const SAMPLES: &[usize] = &[1usize];

#[derive(Default, Clone)]
pub struct FakeStore {
    headers: HashMap<Byte32, HeaderView>,
    total_uncles_count: HashMap<Byte32, u64>,
}

impl FakeStore {
    fn insert(&mut self, block: BlockView) {
        let before_total_uncles_count = self
            .total_uncles_count
            .get(&block.parent_hash())
            .cloned()
            .unwrap_or(0u64);
        self.total_uncles_count.insert(
            block.hash(),
            before_total_uncles_count + block.uncles().data().len() as u64,
        );
        self.headers.insert(block.hash(), block.header());
    }

    pub(crate) fn get_block_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.headers.get(hash).cloned()
    }

    pub(crate) fn total_uncles_count(&self, hash: &Byte32) -> Option<u64> {
        self.total_uncles_count.get(hash).cloned()
    }
}

fn gen_empty_block(parent: &HeaderView) -> BlockView {
    let mut rng = thread_rng();
    let nonce: u128 = rng.gen();
    let uncles_count: u32 = rng.gen_range(0, 2);
    let uncles: Vec<_> = (0..uncles_count)
        .map(|_| {
            BlockBuilder::default()
                .nonce(nonce.pack())
                .build()
                .as_uncle()
        })
        .collect();
    BlockBuilder::default()
        .parent_hash(parent.hash().to_owned())
        .number((parent.number() + 1).pack())
        .uncles(uncles)
        .compact_target(parent.compact_target().pack())
        .timestamp((parent.timestamp() + MIN_BLOCK_INTERVAL * 1000).pack())
        .build()
}

fn bench(c: &mut Criterion) {
    c.bench_function_over_inputs(
        "next_epoch_ext",
        |b, samples| {
            b.iter_with_setup(
                || {
                    let now = unix_time_as_millis();
                    let header = HeaderBuilder::default()
                        .compact_target(GENESIS_TARGET.pack())
                        .timestamp(now.pack())
                        .build();

                    let input = CellInput::new_cellbase_input(0);
                    let witness = Script::default().into_witness(HeaderContextType::NoneContext);
                    let cellbase = TransactionBuilder::default()
                        .input(input)
                        .witness(witness)
                        .build();
                    let dao = genesis_dao_data(vec![&cellbase]).unwrap();
                    let genesis_block = BlockBuilder::default()
                        .compact_target(DIFF_TWO.pack())
                        .dao(dao)
                        .transaction(cellbase)
                        .header(header)
                        .build();

                    let mut parent = genesis_block.header().clone();
                    let epoch_ext =
                        build_genesis_epoch_ext(DEFAULT_EPOCH_REWARD, DIFF_TWO, 1000, 14400);
                    let consensus = ConsensusBuilder::new(genesis_block.clone(), epoch_ext)
                        .initial_primary_epoch_reward(DEFAULT_EPOCH_REWARD)
                        .build();
                    let genesis_epoch_ext = consensus.genesis_epoch_ext().clone();

                    let mut store = FakeStore::default();

                    store.insert(genesis_block);
                    for _ in 1..genesis_epoch_ext.length() {
                        let block = gen_empty_block(&parent);
                        parent = block.header();
                        store.insert(block);
                    }

                    (consensus, genesis_epoch_ext, parent, store)
                },
                |(consensus, genesis_epoch_ext, parent, store)| {
                    let get_block_header = |hash: &Byte32| store.get_block_header(hash);

                    let total_uncles_count = |hash: &Byte32| store.total_uncles_count(hash);

                    for _ in 0..=**samples {
                        consensus.next_epoch_ext(
                            &genesis_epoch_ext,
                            &parent,
                            get_block_header,
                            total_uncles_count,
                        );
                    }
                },
            )
        },
        SAMPLES,
    );
}

criterion_group!(next_epoch_ext, bench);
