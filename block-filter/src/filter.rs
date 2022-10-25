use ckb_async_runtime::tokio::{self, sync::oneshot, task::block_in_place};
use ckb_logger::debug;
use ckb_shared::Shared;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::ChainStore;
use ckb_types::{core::HeaderView, prelude::*};
use golomb_coded_set::{GCSFilterWriter, SipHasher24Builder, M, P};

const NAME: &str = "BlockFilter";

/// A block filter creation service
#[derive(Clone)]
pub struct BlockFilter {
    shared: Shared,
}

impl BlockFilter {
    /// Create a new block filter service
    pub fn new(shared: Shared) -> Self {
        Self { shared }
    }

    /// start background single-threaded service to create block filter data
    pub fn start(self) -> StopHandler<()> {
        let notify_controller = self.shared.notify_controller().clone();
        let async_handle = self.shared.async_handle().clone();
        let (stop, mut stop_rx) = oneshot::channel::<()>();
        let filter_data_builder = self.clone();

        let build_filter_data =
            async_handle.spawn_blocking(move || filter_data_builder.build_filter_data());

        async_handle.spawn(async move {
            let mut new_block_receiver = notify_controller
                .subscribe_new_block(NAME.to_string())
                .await;
            let _build_filter_data_finished = build_filter_data.await;

            loop {
                tokio::select! {
                    Some(_) = new_block_receiver.recv() => {
                        block_in_place(|| self.build_filter_data());
                    }
                    _ = &mut stop_rx => break,
                    else => break,
                }
            }
        });
        StopHandler::new(SignalSender::Tokio(stop), None, NAME.to_string())
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
        self.shared.refresh_snapshot();
    }

    fn build_filter_data_for_block(&self, header: &HeaderView) {
        debug!(
            "Start building filter data for block: {}, hash: {:#x}",
            header.number(),
            header.hash()
        );
        let db = self.shared.store();
        if db.get_block_filter(&header.hash()).is_some() {
            debug!(
                "Filter data for block {:#x} already exist, skip build",
                header.hash()
            );
            return;
        }
        let mut filter_writer = std::io::Cursor::new(Vec::new());
        let mut filter = build_gcs_filter(&mut filter_writer);
        let transactions = db.get_block_body(&header.hash());
        let transactions_size: usize = transactions.iter().map(|tx| tx.data().total_size()).sum();

        for tx in transactions {
            if !tx.is_cellbase() {
                for out_point in tx.input_pts_iter() {
                    let input_cell = db
                        .get_transaction(&out_point.tx_hash())
                        .expect("stored transaction")
                        .0
                        .outputs()
                        .get(out_point.index().unpack())
                        .expect("stored output");
                    filter.add_element(input_cell.calc_lock_hash().as_slice());
                    if let Some(type_script) = input_cell.type_().to_opt() {
                        filter.add_element(type_script.calc_script_hash().as_slice());
                    }
                }
            }
            for output_cell in tx.outputs() {
                filter.add_element(output_cell.calc_lock_hash().as_slice());
                if let Some(type_script) = output_cell.type_().to_opt() {
                    filter.add_element(type_script.calc_script_hash().as_slice());
                }
            }
        }
        filter
            .finish()
            .expect("flush to memory writer should be OK");
        let filter_data = filter_writer.into_inner();
        let db_transaction = db.begin_transaction();
        db_transaction
            .insert_block_filter(&header.hash(), &filter_data.pack())
            .expect("insert_block_filter should be ok");
        db_transaction.commit().expect("commit should be ok");
        debug!("Inserted filter data for block: {}, hash: {:#x}, filter data size: {}, transactions size: {}", header.number(), header.hash(), filter_data.len(), transactions_size);
    }
}

fn build_gcs_filter(out: &mut dyn std::io::Write) -> GCSFilterWriter<SipHasher24Builder> {
    GCSFilterWriter::new(out, SipHasher24Builder::new(0, 0), M, P)
}
