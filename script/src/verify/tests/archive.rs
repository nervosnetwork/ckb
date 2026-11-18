use crate::{TransactionScriptsVerifier, TxVerifyEnv};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{ChainDB, Freezer, StoreSnapshot, data_loader_wrapper::AsDataLoader};
use ckb_test_chain_utils::archive::{
    ArchiveFault, archive_and_collect, assert_cold, assert_hot, collect_published_archive,
    write_segmented_archive,
};
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockBuilder, BlockExt, BlockView, EpochNumberWithFraction, ScriptHashType,
        TransactionBuilder, TransactionInfo,
        cell::{CellMetaBuilder, ResolvedTransaction},
        hardfork::HardForks,
    },
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::Arc,
};

struct ExtensionCorpus {
    blocks: Vec<BlockView>,
    transaction: Arc<ResolvedTransaction>,
    consensus: Arc<Consensus>,
    env: Arc<TxVerifyEnv>,
}

impl ExtensionCorpus {
    fn new() -> Self {
        let program = Bytes::from_static(include_bytes!("../../../testdata/archive_extension"));
        let lock = Script::new_builder()
            .code_hash(CellOutput::calc_data_hash(&program))
            .hash_type(ScriptHashType::Data2)
            .build();
        let code = TransactionBuilder::default()
            .output(
                CellOutput::new_builder()
                    .capacity(100_000_000_000u64)
                    .build(),
            )
            .output_data(program.clone())
            .build();
        let funding = TransactionBuilder::default()
            .output(
                CellOutput::new_builder()
                    .capacity(100_000_000_000u64)
                    .lock(lock)
                    .build(),
            )
            .output_data(Bytes::new())
            .build();
        let archived = BlockBuilder::default()
            .number(1)
            .epoch(EpochNumberWithFraction::new(1, 0, 10))
            .transaction(code.clone())
            .transaction(funding.clone())
            .extension(Some(vec![0x7f; 96].into()))
            .build();
        let absent = BlockBuilder::default()
            .number(2)
            .epoch(EpochNumberWithFraction::new(1, 1, 10))
            .parent_hash(archived.hash())
            .build();
        let side = archived
            .as_advanced_builder()
            .nonce(1u128)
            .extension(Some(vec![0xa5; 17].into()))
            .build();
        let cell = |tx: &ckb_types::core::TransactionView, index, data| {
            CellMetaBuilder::from_cell_output(tx.outputs().get(0).unwrap(), data)
                .out_point(OutPoint::new(tx.hash(), 0))
                .transaction_info(TransactionInfo::new(
                    archived.number(),
                    archived.epoch(),
                    archived.hash(),
                    index,
                ))
                .build()
        };
        let transaction = Arc::new(ResolvedTransaction {
            transaction: TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(funding.hash(), 0), 0))
                .cell_dep(
                    CellDep::new_builder()
                        .out_point(OutPoint::new(code.hash(), 0))
                        .build(),
                )
                .header_deps(vec![archived.hash(), absent.hash(), side.hash()])
                .build(),
            resolved_inputs: vec![cell(&funding, 1, Bytes::new())],
            resolved_cell_deps: vec![cell(&code, 0, program)],
            resolved_dep_groups: vec![],
        });
        Self {
            env: Arc::new(TxVerifyEnv::new_commit(&absent.header())),
            consensus: Arc::new(
                ConsensusBuilder::default()
                    .hardfork_switch(HardForks::new_dev())
                    .build(),
            ),
            transaction,
            blocks: vec![archived, absent, side],
        }
    }

    fn populate(&self, store: &ChainDB) {
        let txn = store.begin_transaction();
        for block in &self.blocks {
            txn.insert_block(block).unwrap();
            txn.insert_block_ext(
                &block.hash(),
                &BlockExt {
                    received_at: 0,
                    total_difficulty: block.difficulty(),
                    total_uncles_count: 0,
                    verified: Some(true),
                    txs_fees: vec![],
                    cycles: None,
                    txs_sizes: None,
                },
            )
            .unwrap();
        }
        txn.attach_block(&self.blocks[0]).unwrap();
        txn.attach_block(&self.blocks[1]).unwrap();
        txn.commit().unwrap();
    }

    fn verify(&self, snapshot: StoreSnapshot) -> Result<u64, ckb_error::Error> {
        TransactionScriptsVerifier::new(
            Arc::clone(&self.transaction),
            Arc::new(snapshot).as_data_loader(),
            Arc::clone(&self.consensus),
            Arc::clone(&self.env),
        )
        .verify(1_000_000)
    }
}

fn open_archive(directory: &Path) -> ChainDB {
    std::fs::create_dir_all(directory).unwrap();
    ChainDB::new_with_freezer(
        RocksDB::open_in(directory.join("db"), COLUMNS),
        Freezer::open_in(directory.join("archive")).unwrap(),
        Default::default(),
    )
}

#[tokio::test]
async fn archived_extensions_execute_in_vm_before_and_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let corpus = ExtensionCorpus::new();
    let hot = ChainDB::new(
        RocksDB::open_in(directory.path().join("hot"), COLUMNS),
        Default::default(),
    );
    let archive_path = directory.path().join("cold");
    let cold = open_archive(&archive_path);
    for store in [&hot, &cold] {
        corpus.populate(store);
    }
    assert_hot(&hot, &corpus.blocks);
    let expected = corpus.verify(hot.get_snapshot()).unwrap();
    let before = cold.get_snapshot();
    assert_eq!(corpus.verify(cold.get_snapshot()).unwrap(), expected);
    archive_and_collect(&cold, &corpus.blocks).await;
    assert_eq!(corpus.verify(cold.get_snapshot()).unwrap(), expected);
    assert_eq!(corpus.verify(before).unwrap(), expected);
    drop(cold);

    let cold = open_archive(&archive_path);
    cold.recover_archive().unwrap();
    assert_cold(&cold, &corpus.blocks);
    assert_hot(&hot, &corpus.blocks);
    assert_eq!(
        corpus.verify(cold.get_snapshot()).unwrap(),
        corpus.verify(hot.get_snapshot()).unwrap()
    );
}

#[test]
fn archived_storage_faults_do_not_become_vm_missing_items() {
    let reference = tempfile::tempdir().unwrap();
    let corpus = ExtensionCorpus::new();
    let hot = ChainDB::new(
        RocksDB::open_in(reference.path(), COLUMNS),
        Default::default(),
    );
    corpus.populate(&hot);
    let expected = corpus.verify(hot.get_snapshot()).unwrap();

    for fault in ArchiveFault::ALL {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("archive");
        write_segmented_archive(&archive, &corpus.blocks);
        let cold = open_archive(directory.path());
        corpus.populate(&cold);
        collect_published_archive(&cold, &corpus.blocks);
        assert_eq!(corpus.verify(cold.get_snapshot()).unwrap(), expected);
        drop(cold);

        let cold = open_archive(directory.path());
        assert_cold(&cold, &corpus.blocks);
        fault.inject(&cold, &archive, &corpus.blocks[0], &corpus.blocks[2]);
        // Catch only the consumer call: an ordinary VM error is not a storage failure.
        let result = catch_unwind(AssertUnwindSafe(|| corpus.verify(cold.get_snapshot())));
        fault.assert_failure(result.expect_err("storage fault returned a normal VM result"));
        assert_hot(&hot, &corpus.blocks);
        assert_eq!(corpus.verify(hot.get_snapshot()).unwrap(), expected);
    }
}
