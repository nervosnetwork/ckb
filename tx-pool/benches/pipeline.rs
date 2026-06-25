//! Criterion benchmark entry point for the tx-pool pipeline.
//!
//! Run with:
//!
//!   cargo bench -p ckb-tx-pool --features internal --bench pipeline
//!
//! To benchmark the synchronous (pre-pipeline) baseline instead, disable the
//! default `pipeline` feature:
//!
//!   cargo bench -p ckb-tx-pool --no-default-features --features internal --bench pipeline

use ckb_tx_pool::benchmark::pipeline_bench;
use criterion::criterion_main;

criterion_main!(pipeline_bench);
