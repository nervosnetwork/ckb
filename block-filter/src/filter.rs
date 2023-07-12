use ckb_async_runtime::tokio::{self, task::block_in_place};
use ckb_logger::{debug, warn};
use ckb_shared::Shared;
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    core::HeaderView,
    packed::{Byte32, CellOutput, OutPoint},
    prelude::*,
    utilities::{build_filter_data, FilterDataProvider},
};

const NAME: &str = "BlockFilter";

/// A block filter creation service
#[derive(Clone)]
pub struct BlockFilter {
    shared: Shared,
}

struct WrappedChainDB<'a> {
    inner: &'a ChainDB,
}

impl<'a> FilterDataProvider for WrappedChainDB<'a> {
    fn cell(&self, out_point: &OutPoint) -> Option<CellOutput> {
        self.inner
            .get_transaction(&out_point.tx_hash())
            .and_then(|(tx, _)| tx.outputs().get(out_point.index().unpack()))
    }
}

impl<'a> WrappedChainDB<'a> {
    fn new(inner: &'a ChainDB) -> Self {
        Self { inner }
    }
}

impl BlockFilter {
    /// Create a new block filter service
    pub fn new(shared: Shared) -> Self {
        Self { shared }
    }

    /// start background single-threaded service to create block filter data
    pub fn start(self) {
        let notify_controller = self.shared.notify_controller().clone();
        let async_handle = self.shared.async_handle().clone();
        let stop_rx: CancellationToken = new_tokio_exit_rx();
        let filter_data_builder = self.clone();

        let build_filter_data =
            async_handle.spawn_blocking(move || filter_data_builder.build_filter_data());

        async_handle.spawn(async move {
            let mut new_block_watcher = notify_controller.watch_new_block(NAME.to_string()).await;
            let _build_filter_data_finished = build_filter_data.await;

            loop {
                tokio::select! {
                    Ok(_) = new_block_watcher.changed() => {
                        block_in_place(|| self.build_filter_data());
                        new_block_watcher.borrow_and_update();
                    }
                    _ = stop_rx.cancelled() => {
                        debug!("BlockFilter received exit signal, exit now");
                        break
                    },
                    else => break,
                }
            }
        });
    }

    /// build block filter data to the latest block
    fn build_filter_data(&self) {
        let snapshot = self.shared.snapshot();
        let tip_header = snapshot.get_tip_header().expect("tip stored");
        let start_number = match snapshot.get_latest_built_filter_data_block_hash() {
            Some(block_hash) => {
                debug!("Latest built block hash {:#x}", block_hash);
                if snapshot.is_main_chain(&block_hash) {
                    let header = snapshot
                        .get_block_header(&block_hash)
                        .expect("header stored");
                    debug!(
                        "Latest built block is main chain, start from {}",
                        header.number() + 1
                    );
                    header.number() + 1
                } else {
                    // find fork chain number
                    let mut header = snapshot
                        .get_block_header(&block_hash)
                        .expect("header stored");
                    while !snapshot.is_main_chain(&header.parent_hash()) {
                        header = snapshot
                            .get_block_header(&header.parent_hash())
                            .expect("parent header stored");
                    }
                    debug!(
                        "Latest built filter data block is fork chain, start from {}",
                        header.number()
                    );
                    header.number()
                }
            }
            None => 0,
        };

        for block_number in start_number..=tip_header.number() {
            let block_hash = snapshot.get_block_hash(block_number).expect("index stored");
            let header = snapshot
                .get_block_header(&block_hash)
                .expect("header stored");
            self.build_filter_data_for_block(&header);
        }
    }

    fn build_filter_data_for_block(&self, header: &HeaderView) {
        debug!(
            "Start building filter data for block: {}, hash: {:#x}",
            header.number(),
            header.hash()
        );
        let db = self.shared.store();
        if db.get_block_filter_hash(&header.hash()).is_some() {
            debug!(
                "Filter data for block {:#x} already exist, skip build",
                header.hash()
            );
            return;
        }
        let parent_block_filter_hash = if header.is_genesis() {
            Byte32::zero()
        } else {
            db.get_block_filter_hash(&header.parent_hash())
                .expect("parent block filter data stored")
        };

        let transactions = db.get_block_body(&header.hash());
        let transactions_size: usize = transactions.iter().map(|tx| tx.data().total_size()).sum();
        let provider = WrappedChainDB::new(db);
        let (filter_data, missing_out_points) = build_filter_data(provider, &transactions);
        for out_point in missing_out_points {
            warn!(
                "Can't find input cell for out_point: {:#x}, \
                should only happen in test, skip adding to filter",
                out_point
            );
        }
        let db_transaction = db.begin_transaction();
        db_transaction
            .insert_block_filter(
                &header.hash(),
                &filter_data.pack(),
                &parent_block_filter_hash,
            )
            .expect("insert_block_filter should be ok");
        db_transaction.commit().expect("commit should be ok");
        debug!("Inserted filter data for block: {}, hash: {:#x}, filter data size: {}, transactions size: {}", header.number(), header.hash(), filter_data.len(), transactions_size);
    }
}
