use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_db::RocksDB;
use ckb_db_schema::{COLUMN_BLOCK_HEADER, COLUMNS};
use ckb_freezer::Freezer;
use ckb_types::{core::BlockExt, packed, prelude::*};
use tempfile::TempDir;

use crate::{db::ChainDB, store::ChainStore};

#[test]
fn save_and_get_block() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block(block).unwrap();
    txn.commit().unwrap();
    assert_eq!(block, &store.get_block(&hash).unwrap());
}

#[test]
fn save_and_get_block_with_transactions() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let block = packed::Block::new_builder()
        .transactions(
            (0..3)
                .map(|_| packed::Transaction::new_builder().build())
                .collect::<Vec<_>>(),
        )
        .build()
        .into_view();

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.commit().unwrap();
    assert_eq!(block, store.get_block(&hash).unwrap());
}

#[test]
fn save_and_get_block_ext() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();

    let ext = BlockExt {
        received_at: block.timestamp(),
        total_difficulty: block.difficulty(),
        total_uncles_count: block.data().uncles().len() as u64,
        verified: Some(true),
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block_ext(&hash, &ext).unwrap();
    txn.commit().unwrap();
    assert_eq!(ext, store.get_block_ext(&hash).unwrap());
}

#[test]
fn index_store() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let consensus = ConsensusBuilder::default().build();
    let block = consensus.genesis_block();
    let hash = block.hash();
    store.init(&consensus).unwrap();
    assert_eq!(hash, store.get_block_hash(0).unwrap());

    assert_eq!(
        block.difficulty(),
        store.get_block_ext(&hash).unwrap().total_difficulty
    );

    assert_eq!(block.number(), store.get_block_number(&hash).unwrap());

    assert_eq!(block.header(), store.get_tip_header().unwrap());
}

#[test]
fn attached_cells_preserve_metadata_across_blocks() {
    use ckb_types::{
        bytes::Bytes,
        core::{BlockBuilder, EpochNumberWithFraction, TransactionBuilder, TransactionInfo},
    };

    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());
    let blocks: Vec<_> = [
        (17, EpochNumberWithFraction::new(4, 2, 10)),
        (99, EpochNumberWithFraction::new(9, 3, 20)),
    ]
    .into_iter()
    .map(|(number, epoch)| {
        let transactions = [0, 1, 3].into_iter().enumerate().map(|(index, outputs)| {
            TransactionBuilder::default()
                .version(index as u32)
                .input(packed::CellInput::new_cellbase_input(number))
                .outputs((0..outputs).map(|output| {
                    packed::CellOutput::new_builder()
                        .capacity(number.saturating_add(output as u64))
                        .build()
                }))
                .outputs_data((0..outputs).map(|output| Bytes::from(vec![7; output]).pack()))
                .build()
        });
        BlockBuilder::default()
            .number(number)
            .epoch(epoch)
            .transactions(transactions)
            .build()
    })
    .collect();
    let txn = store.begin_transaction();
    for block in &blocks {
        txn.insert_block(block).unwrap();
        txn.attach_block(block).unwrap();
        crate::attach_block_cell(&txn, block).unwrap();
    }
    txn.commit().unwrap();

    for block in &blocks {
        for (index, transaction) in block.transactions().iter().enumerate() {
            let info = TransactionInfo {
                block_number: block.number(),
                block_epoch: block.epoch(),
                block_hash: block.hash(),
                index,
            };
            assert_eq!(
                store.get_transaction_info(&transaction.hash()),
                Some(info.clone())
            );
            for (output_index, (output, data)) in transaction.outputs_with_data_iter().enumerate() {
                let point = packed::OutPoint::new(transaction.hash(), output_index as u32);
                let cell = store.get_cell(&point).unwrap();
                assert_eq!(cell.cell_output, output);
                assert_eq!(cell.transaction_info, Some(info.clone()));
                assert_eq!(cell.data_bytes, data.len() as u64);
                let data_hash = packed::CellOutput::calc_data_hash(&data);
                assert_eq!(store.get_cell_data(&point), Some((data, data_hash.clone())));
                assert_eq!(store.get_cell_data_hash(&point), Some(data_hash));
            }
        }
    }
}

#[test]
fn attached_cell_order_preserves_indices_overwrites_and_cellbase() {
    use ckb_db_schema::{COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH};
    use ckb_types::{
        bytes::Bytes,
        core::{BlockBuilder, EpochNumberWithFraction, TransactionBuilder},
    };

    let tmp_dir = TempDir::new().unwrap();
    let store = ChainDB::new(RocksDB::open_in(&tmp_dir, COLUMNS), Default::default());
    let cellbase = TransactionBuilder::default()
        .input(packed::CellInput::new_cellbase_input(1))
        .output(packed::CellOutput::default())
        .output_data(Bytes::new())
        .build();
    let mut ordinary: Vec<_> = (0..300u32)
        .map(|index| {
            TransactionBuilder::default()
                .input(packed::CellInput::new(
                    packed::OutPoint::new(packed::Byte32::new([7; 32]), index),
                    0,
                ))
                .output(packed::CellOutput::default())
                .output_data(Bytes::from(vec![1; index as usize % 3]))
                .build()
        })
        .collect();
    ordinary.sort_by(|left, right| right.hash_ref().cmp(left.hash_ref()));
    assert!(ordinary.last().unwrap().hash_ref() < cellbase.hash_ref());
    let repeated = ordinary[0].clone();
    let same_block_spent = packed::OutPoint::new(ordinary[1].hash(), 0);
    let consumer = TransactionBuilder::default()
        .input(packed::CellInput::new(same_block_spent.clone(), 0))
        .output(packed::CellOutput::default())
        .output_data(Bytes::new())
        .build();
    let block = BlockBuilder::default()
        .number(1u64)
        .epoch(EpochNumberWithFraction::new(0, 1, 10))
        .transaction(cellbase)
        .transactions(ordinary)
        .transaction(repeated.clone())
        .transaction(consumer)
        .build();
    let txn = store.begin_transaction();
    // A cellbase's null input must never be deleted, even if its hash sorts
    // after ordinary transactions. Ordinary inputs still become dead.
    let null = packed::OutPoint::null();
    let spent: Vec<_> = (0..300)
        .map(|index| packed::OutPoint::new(packed::Byte32::new([7; 32]), index))
        .collect();
    let data = Bytes::from_static(b"spent");
    let entry = packed::CellEntry::new_builder()
        .data_size(data.len() as u64)
        .build();
    let data_entry = packed::CellDataEntry::new_builder()
        .output_data_hash(packed::CellOutput::calc_data_hash(&data))
        .output_data(data)
        .build();
    txn.insert_cells(
        std::iter::once(&null)
            .chain(&spent)
            .map(|point| (point.clone(), entry.clone(), Some(data_entry.clone()))),
    )
    .unwrap();
    txn.commit().unwrap();
    let txn = store.begin_transaction();
    txn.attach_block(&block).unwrap();
    crate::attach_block_cell(&txn, &block).unwrap();
    assert!(txn.get_cell(&null).is_some());
    for point in &spent {
        for column in [COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH] {
            assert!(store.get(column, &point.to_cell_key()).is_some());
            assert!(txn.get(column, &point.to_cell_key()).is_none());
        }
    }
    let transactions = block.transactions();
    for (index, transaction) in transactions.iter().enumerate() {
        let expected = if transaction.hash() == repeated.hash() {
            transactions.len() - 2
        } else {
            index
        };
        let info = txn.get_transaction_info(&transaction.hash()).unwrap();
        assert_eq!(info.index, expected);
        assert_eq!(info.block_hash, block.hash());
        assert_eq!(info.block_number, block.number());
        assert_eq!(info.block_epoch, block.epoch());
        assert!(store.get_transaction_info(&transaction.hash()).is_none());
        let point = packed::OutPoint::new(transaction.hash(), 0);
        if point == same_block_spent {
            for column in [COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH] {
                assert!(txn.get(column, &point.to_cell_key()).is_none());
            }
            continue;
        }
        let cell = txn.get_cell(&point).unwrap();
        assert_eq!(cell.transaction_info.unwrap().index, expected);
        assert_eq!(
            txn.get_cell_data(&point).unwrap().0,
            transaction.outputs_data().get(0).unwrap().raw_data()
        );
    }
    txn.commit().unwrap();
    assert!(store.get_cell(&null).is_some());
    for point in spent.iter().chain(std::iter::once(&same_block_spent)) {
        for column in [COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH] {
            assert!(store.get(column, &point.to_cell_key()).is_none());
        }
    }
}

#[test]
fn cell_batches_preserve_overwrites_and_transaction_isolation() {
    use ckb_db_schema::{COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH};
    use ckb_types::bytes::Bytes;

    let tmp_dir = TempDir::new().unwrap();
    let store = ChainDB::new(RocksDB::open_in(&tmp_dir, COLUMNS), Default::default());
    let cell = |index: u32, capacity: u64, data: &[u8]| {
        let point = packed::OutPoint::new(packed::Byte32::zero(), index);
        let entry = packed::CellEntry::new_builder()
            .output(packed::CellOutput::new_builder().capacity(capacity).build())
            .data_size(data.len() as u64)
            .build();
        let data = Bytes::copy_from_slice(data);
        let data_entry = (!data.is_empty()).then(|| {
            packed::CellDataEntry::new_builder()
                .output_data_hash(packed::CellOutput::calc_data_hash(&data))
                .output_data(data)
                .build()
        });
        (point, entry, data_entry)
    };
    let original = cell(7, 70, b"original");
    let txn = store.begin_transaction();
    txn.insert_cells(std::iter::once(original.clone())).unwrap();
    txn.commit().unwrap();
    let snapshot = store.get_snapshot();

    let mut rows: Vec<_> = (0..600)
        .rev()
        .map(|index| cell(index, 1000 + u64::from(index), b"ordinary"))
        .collect();
    rows.insert(50, cell(579, 7000, b""));
    rows.insert(51, cell(579, 7001, b"replacement"));
    rows.insert(52, cell(7, 8000, b"early batch"));
    let large_data = vec![5; 128 * 1024];
    rows.insert(53, cell(570, 7002, &large_data));
    rows.push(cell(7, 9000, b""));

    let aborted = store.begin_transaction();
    aborted.insert_cells(rows.clone().into_iter()).unwrap();
    drop(aborted);
    assert_eq!(
        store.get(COLUMN_CELL, &original.0.to_cell_key()).as_deref(),
        Some(original.1.as_slice())
    );
    assert!(store.get_cell(&rows[0].0).is_none());

    let txn = store.begin_transaction();
    txn.insert_cells(rows.into_iter()).unwrap();
    for index in 0..600 {
        let expected = match index {
            579 => cell(index, 7001, b"replacement"),
            570 => cell(index, 7002, &large_data),
            7 => cell(index, 9000, b""),
            _ => cell(index, 1000 + u64::from(index), b"ordinary"),
        };
        let key = expected.0.to_cell_key();
        assert_eq!(
            txn.get(COLUMN_CELL, &key).as_deref(),
            Some(expected.1.as_slice())
        );
        assert_eq!(
            txn.get(COLUMN_CELL_DATA, &key).as_deref(),
            Some(expected.2.as_ref().map_or(&[][..], Entity::as_slice))
        );
        let hash = expected.2.as_ref().map(|entry| entry.output_data_hash());
        assert_eq!(
            txn.get(COLUMN_CELL_DATA_HASH, &key).as_deref(),
            Some(hash.as_ref().map_or(&[][..], Entity::as_slice))
        );
    }
    txn.commit().unwrap();
    assert_eq!(
        snapshot
            .get(COLUMN_CELL, &original.0.to_cell_key())
            .as_deref(),
        Some(original.1.as_slice())
    );

    let txn = store.begin_transaction();
    txn.delete_cells(
        (0..600)
            .rev()
            .chain([7, 579])
            .map(|index| packed::OutPoint::new(packed::Byte32::zero(), index)),
    )
    .unwrap();
    txn.commit().unwrap();
    for index in 0..600 {
        let key = packed::OutPoint::new(packed::Byte32::zero(), index).to_cell_key();
        for column in [COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH] {
            assert!(store.get(column, &key).is_none());
        }
    }
}

#[test]
fn cell_writes_stop_after_the_first_iterator_end() {
    let tmp_dir = TempDir::new().unwrap();
    let store = ChainDB::new(RocksDB::open_in(&tmp_dir, COLUMNS), Default::default());
    let first = packed::OutPoint::new(packed::Byte32::zero(), 0);
    let second = packed::OutPoint::new(packed::Byte32::zero(), 1);
    let mut cells = [
        Some((first.clone(), packed::CellEntry::default(), None)),
        None,
        Some((second.clone(), packed::CellEntry::default(), None)),
    ]
    .into_iter();
    let txn = store.begin_transaction();
    txn.insert_cells(std::iter::from_fn(|| cells.next().flatten()))
        .unwrap();
    assert!(txn.get_cell(&first).is_some());
    assert!(txn.get_cell(&second).is_none());
    txn.insert_cells(std::iter::once((
        second.clone(),
        packed::CellEntry::default(),
        None,
    )))
    .unwrap();
    let mut points = [Some(first.clone()), None, Some(second.clone())].into_iter();
    txn.delete_cells(std::iter::from_fn(|| points.next().flatten()))
        .unwrap();
    assert!(txn.get_cell(&first).is_none());
    assert!(txn.get_cell(&second).is_some());
    txn.commit().unwrap();
    assert!(store.get_cell(&first).is_none());
    assert!(store.get_cell(&second).is_some());
}

#[test]
fn freeze_blockv0() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let raw = packed::RawHeader::new_builder().number(1u64).build();
    let block = packed::Block::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .build()
        .into_view();

    let block_hash = block.hash();
    let header = block.header();

    let txn = store.begin_transaction();
    txn.insert_raw(
        COLUMN_BLOCK_HEADER,
        block_hash.as_slice(),
        Into::<packed::HeaderView>::into(header).as_slice(),
    )
    .expect("insert header");
    txn.commit().expect("commit");

    freezer
        .freeze(2, |_number| Some(block.clone()))
        .expect("freeze");

    assert_eq!(store.get_block(&block_hash), Some(block));
}

#[test]
fn freeze_blockv1_with_extension() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let extension: packed::Bytes = [1u8; 96].into();
    let raw = packed::RawHeader::new_builder().number(1u64).build();
    let block = packed::BlockV1::new_builder()
        .header(packed::Header::new_builder().raw(raw).build())
        .extension(extension)
        .build()
        .as_v0()
        .into_view();

    let block_hash = block.hash();
    let header = block.header();

    let txn = store.begin_transaction();
    txn.insert_raw(
        COLUMN_BLOCK_HEADER,
        block_hash.as_slice(),
        Into::<packed::HeaderView>::into(header).as_slice(),
    )
    .expect("insert header");
    txn.commit().expect("commit");

    freezer
        .freeze(2, |_number| Some(block.clone()))
        .expect("freeze");

    let block = store.get_block(&block_hash).expect("get_block");
    assert_eq!(store.get_block(&block_hash), Some(block));
}

#[test]
fn freezer_get_block_keeps_hash_lookup_contract_for_same_height_side_block() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let frozen_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(1u128)
                .build(),
        )
        .build()
        .into_view();
    let side_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(2u128)
                .build(),
        )
        .build()
        .into_view();
    let side_hash = side_block.hash();
    assert_ne!(frozen_block.hash(), side_hash);

    let txn = store.begin_transaction();
    txn.insert_block(&frozen_block).unwrap();
    txn.insert_block(&side_block).unwrap();
    txn.commit().unwrap();

    freezer
        .freeze(2, |_number| Some(frozen_block.clone()))
        .expect("freeze");

    assert_eq!(store.get_block(&side_hash), Some(side_block));
}

#[test]
fn freezer_get_transaction_keeps_hash_lookup_contract_for_same_height_side_tx() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let tmp_dir2 = TempDir::new().unwrap();
    let freezer = Freezer::open_in(&tmp_dir2).expect("tmp freezer");
    let store = ChainDB::new_with_freezer(db, freezer.clone(), Default::default());

    let frozen_tx = packed::Transaction::new_builder()
        .raw(packed::RawTransaction::new_builder().version(1u32).build())
        .build()
        .into_view();
    let side_tx = packed::Transaction::new_builder()
        .raw(packed::RawTransaction::new_builder().version(2u32).build())
        .build()
        .into_view();
    let side_tx_hash = side_tx.hash();
    assert_ne!(frozen_tx.hash(), side_tx_hash);

    let frozen_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(1u128)
                .build(),
        )
        .transactions(vec![frozen_tx.data()])
        .build();
    let frozen_block = frozen_block.into_view();
    let side_block = packed::Block::new_builder()
        .header(
            packed::Header::new_builder()
                .raw(packed::RawHeader::new_builder().number(1u64).build())
                .nonce(2u128)
                .build(),
        )
        .transactions(vec![side_tx.data()])
        .build();
    let side_block = side_block.into_view();
    let side_block_hash = side_block.hash();
    assert_ne!(frozen_block.hash(), side_block_hash);

    let txn = store.begin_transaction();
    txn.insert_block(&frozen_block).unwrap();
    txn.insert_block(&side_block).unwrap();
    txn.attach_block(&side_block).unwrap();
    txn.commit().unwrap();

    freezer
        .freeze(2, |_number| Some(frozen_block.clone()))
        .expect("freeze");

    let (tx, tx_info) = store
        .get_transaction_with_info(&side_tx_hash)
        .expect("get side transaction");
    assert_eq!(tx, side_tx);
    assert_eq!(tx_info.block_hash, side_block_hash);
}
