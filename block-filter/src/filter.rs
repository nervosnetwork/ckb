use ckb_channel::select;
use ckb_logger::debug;
use ckb_notify::NotifyController;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{core::HeaderView, prelude::*};
use std::thread;

const THREAD_NAME: &str = "ckb-block-filter-thread";

/// A block filter creation service
pub struct BlockFilter {
    db: ChainDB,
}

impl BlockFilter {
    /// Create a new block filter service
    pub fn new(db: ChainDB) -> Self {
        Self { db }
    }

    /// start background single-threaded service to create block filter data
    pub fn start(self, notify_controller: &NotifyController) {
        let thread_builder = thread::Builder::new().name(THREAD_NAME.to_string());

        let notify_controller = notify_controller.clone();
        let _thread = thread_builder
            .spawn(move || {
                // catch up to the latest block
                self.build_filter_data();
                // subscribe to new block
                let new_block_receiver = notify_controller.subscribe_new_block(THREAD_NAME);
                loop {
                    select! {
                        recv(new_block_receiver) -> _ => {
                            self.build_filter_data();
                        }
                    }
                }
            })
            .expect("Start block filter service failed");
    }

    /// build block filter data to the latest block
    fn build_filter_data(&self) {
        let snapshot = self.db.get_snapshot();
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
        let db = &self.db;
        if db.get_block_filter(&header.hash()).is_some() {
            debug!(
                "Filter data for block {:#x} already exist, skip build",
                header.hash()
            );
            return;
        }
        let mut filter_writer = std::io::Cursor::new(Vec::new());
        let mut filter = build_gcs_filter(&mut filter_writer);
        let transcations = db.get_block_body(&header.hash());
        let transactions_size: usize = transcations.iter().map(|tx| tx.data().total_size()).sum();

        for tx in transcations {
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

fn build_gcs_filter(out: &mut dyn std::io::Write) -> golomb_coded_set::GCSFilterWriter {
    // use same value as bip158
    let p = 19;
    let m = 1.497_137 * f64::from(2u32.pow(p));
    golomb_coded_set::GCSFilterWriter::new(out, 0, 0, m as u64, p as u8)
}
