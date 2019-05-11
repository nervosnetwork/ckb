use ckb_core::{
    block::BlockBuilder,
    transaction::{CellOutput, TransactionBuilder},
};
use ckb_db::{DBConfig, RocksDB};
use ckb_store::{ChainKVStore, ChainStore, StoreBatch, COLUMNS};
use criterion::{criterion_group, criterion_main, Criterion};

fn bench(c: &mut Criterion) {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let config = DBConfig {
        path: tmp_dir.as_ref().to_path_buf(),
        ..Default::default()
    };

    let test_data = {
        let db = RocksDB::open(&config, COLUMNS);
        let store = ChainKVStore::new(db);

        let output = CellOutput::default();
        let tx1 = TransactionBuilder::default().output(output.clone()).build();
        let tx50 = TransactionBuilder::default()
            .outputs(vec![output.clone(); 50])
            .build();
        let tx100 = TransactionBuilder::default()
            .outputs(vec![output.clone(); 100])
            .build();
        let tx300 = TransactionBuilder::default()
            .outputs(vec![output.clone(); 300])
            .build();
        let test_data = vec![
            (1, tx1.hash().to_owned()),
            (50, tx50.hash().to_owned()),
            (100, tx100.hash().to_owned()),
            (300, tx300.hash().to_owned()),
        ];
        let txs = vec![tx1, tx50, tx100, tx300];
        let block = BlockBuilder::default().transactions(txs).build();

        let mut batch = store.new_batch().unwrap();
        batch.insert_block(&block).unwrap();
        batch.attach_block(&block).unwrap();
        batch.commit().unwrap();
        test_data
    };

    for data in test_data.into_iter() {
        let db = RocksDB::open(&config, COLUMNS);
        let store = ChainKVStore::new(db);
        let name = format!("fetch_cell_output_from_{}", data.0);
        c.bench_function(&name, move |b| {
            b.iter(|| {
                for idx in 0..data.0 {
                    let _ = store.get_cell_output(&data.1, idx).unwrap();
                }
            })
        });
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
