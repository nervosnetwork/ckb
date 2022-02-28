use ckb_channel::select;
use ckb_logger::debug;
use ckb_notify::NotifyController;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{core::HeaderView, prelude::*};
use std::thread;

const THREAD_NAME: &str = "ckb-block-filter-thread";

pub struct BlockFilter {
    db: ChainDB,
}

impl BlockFilter {
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
        // TODO update the built block number to the META column family
        let mut header = self.db.get_tip_header().expect("tip stored");
        while self.db.get_block_filter(&header.hash()).is_none() {
            self.build_filter_data_for_block(&header);
            if header.is_genesis() {
                break;
            } else {
                header = self
                    .db
                    .get_block_header(&header.parent_hash())
                    .expect("parent header stored");
            }
        }
    }

    fn build_filter_data_for_block(&self, header: &HeaderView) {
        debug!(
            "Start building filter data for block: {}, hash: {}",
            header.number(),
            header.hash()
        );
        let db = &self.db;
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
        db.begin_transaction()
            .insert_block_filter(&header.hash(), &filter_data.pack())
            .expect("insert_block_filter should be ok");
        debug!("Inserted filter data for block: {}, hash: {}, filter data size: {}, transactions size: {}", header.number(), header.hash(), filter_data.len(), transactions_size);
    }
}

fn build_gcs_filter(out: &mut dyn std::io::Write) -> golomb_coded_set::GCSFilterWriter {
    // use same value as bip158
    let p = 19;
    let m = 1.497_137 * f64::from(2u32.pow(p));
    golomb_coded_set::GCSFilterWriter::new(out, 0, 0, m as u64, p as u8)
}
