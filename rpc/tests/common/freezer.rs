use ckb_app_config::IndexerConfig;
use ckb_async_runtime::Handle;
use ckb_indexer::IndexerService;
use ckb_indexer_sync::{IndexerSource, PoolService};
use ckb_jsonrpc_types::{IndexerCell, IndexerOrder, IndexerSearchKey};
use ckb_notify::{NotifyController, NotifyService};
use ckb_rich_indexer::RichIndexerService;
use ckb_store::ChainDB;
use ckb_types::{
    core::{BlockBuilder, BlockExt, BlockView, EpochNumberWithFraction, TransactionBuilder},
    packed,
    prelude::*,
};
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};
pub fn block(parent: Option<&BlockView>, nonce: u8) -> BlockView {
    let number = parent.map_or(0, |p| p.number() + 1);
    let tx = TransactionBuilder::default()
        .input(packed::CellInput::new_cellbase_input(number))
        .output(
            packed::CellOutput::new_builder()
                .capacity(100_000_000_000u64)
                .build(),
        )
        .output_data(packed::Bytes::from(vec![nonce]))
        .witness(packed::Bytes::from(vec![nonce; 32]))
        .build();
    BlockBuilder::default()
        .number(number)
        .epoch(EpochNumberWithFraction::new(14 + number, 0, 1))
        .parent_hash(parent.map(BlockView::hash).unwrap_or_default())
        .transaction(tx)
        .extension(Some(vec![nonce].into()))
        .build()
}

pub fn verified_ext() -> BlockExt {
    BlockExt {
        received_at: 0,
        total_difficulty: Default::default(),
        total_uncles_count: 0,
        verified: Some(true),
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    }
}

pub fn attach(store: &ChainDB, added: &[BlockView], detached: &[BlockView]) {
    let tx = store
        .begin_block_transaction(added.first().unwrap())
        .unwrap();
    for b in detached.iter().rev() {
        tx.detach_block(b).unwrap();
    }
    for b in added {
        tx.insert_block(b).unwrap();
        tx.attach_block(b).unwrap();
        tx.insert_block_ext(&b.hash(), &verified_ext()).unwrap();
    }
    tx.insert_tip_header(&added.last().unwrap().header())
        .unwrap();
    tx.commit().unwrap();
}

pub struct Indexers {
    rt: Handle,
    ordinary: IndexerService,
    rich: RichIndexerService,
    pub notify: NotifyController,
}

impl Indexers {
    pub fn start(root: &Path, primary: &Path, ancient: &Path, rt: &Handle) -> Self {
        let mut config = IndexerConfig {
            poll_interval: 1,
            ..Default::default()
        };
        config.adjust(root, root.join("indexer"));
        std::fs::create_dir_all(root.join("indexer")).unwrap();
        let src = IndexerSource::with_freezer(primary, &config.secondary_path, ancient);
        let pool = PoolService::new(false, rt.clone());
        let ordinary = IndexerService::new(src.clone(), pool.clone(), &config, rt.clone());
        let rich = RichIndexerService::new(src, pool, &config, rt.clone());
        let notify = NotifyService::new(Default::default(), rt.clone()).start();
        ordinary.spawn_poll(notify.clone());
        rich.spawn_poll(notify.clone());
        Self {
            rt: rt.clone(),
            ordinary,
            rich,
            notify,
        }
    }
    pub fn wait_for(&self, expected: &[&BlockView]) {
        let tip = expected.last().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let tips = [
                self.ordinary.handle().get_indexer_tip().unwrap(),
                self.rich.handle().get_indexer_tip().unwrap(),
            ];
            if tips.iter().all(|v| {
                v.as_ref().is_some_and(|v| {
                    v.block_hash == tip.hash().into() && v.block_number.value() == tip.number()
                })
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "indexers did not reach {}: {:?}",
                tip.number(),
                tips.map(|v| v.map(|v| (v.block_number.value(), v.block_hash)))
            );
            thread::sleep(Duration::from_millis(30));
        }
        let plain = self
            .ordinary
            .handle()
            .get_cells(
                IndexerSearchKey::default(),
                IndexerOrder::Asc,
                100u32.into(),
                None,
            )
            .unwrap();
        let sql = self
            .rt
            .block_on(self.rich.async_handle().get_cells(
                IndexerSearchKey::default(),
                IndexerOrder::Asc,
                100u32.into(),
                None,
            ))
            .unwrap();
        let contents = |cells: Vec<IndexerCell>| {
            cells
                .into_iter()
                .map(|c| {
                    (
                        c.block_number.value(),
                        c.out_point.tx_hash,
                        c.output_data.unwrap().into_bytes(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let expected: Vec<_> = expected
            .iter()
            .map(|b| {
                let t = &b.transactions()[0];
                (
                    b.number(),
                    t.hash().into(),
                    t.outputs_data().get(0).unwrap().raw_data(),
                )
            })
            .collect();
        assert_eq!(contents(plain.objects), expected);
        assert_eq!(contents(sql.objects), expected);
    }
    pub fn hold_at(&self, block: &BlockView) {
        let tips = || {
            [
                self.ordinary
                    .handle()
                    .get_indexer_tip()
                    .unwrap()
                    .unwrap()
                    .block_hash,
                self.rich
                    .handle()
                    .get_indexer_tip()
                    .unwrap()
                    .unwrap()
                    .block_hash,
            ]
        };

        // Cover at least two real polling intervals. The later successful read
        // verifies both pollers are still alive after the fault is repaired.
        let until = Instant::now() + Duration::from_millis(2300);
        while Instant::now() < until {
            assert_eq!(tips(), [block.hash().into(), block.hash().into()]);
            thread::sleep(Duration::from_millis(40));
        }
    }
}
