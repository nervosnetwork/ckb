use ckb_error::{Error, InternalErrorKind};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{
    ChainStore, IterDirection, IteratorMode, WriteBatch, COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_META, COLUMN_PRUNED, COLUMN_PRUNE_MASK,
    META_PRUNING_EPOCH_KEY,
};
use ckb_types::{
    core::{service, BlockNumber, EpochExt, EpochNumber, HeaderView},
    packed,
    prelude::*,
};
use std::collections::HashMap;
use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::{cmp, mem, thread};

pub struct PruneController {
    stop: StopHandler<()>,
}

impl PruneController {
    pub fn join(&mut self) {
        self.stop.try_send();
    }
}

pub struct PruneService {
    shared: Shared,
}

pub struct PruningTask {
    have_pruned: bool,
    pruning_epoch: EpochNumber,
    current_epoch_ext: EpochExt,
    snapshot: Arc<Snapshot>,
    shared: Shared,
}

#[derive(Debug, Default)]
pub struct FullDeadTransactions {
    pub(crate) inner: HashMap<(BlockNumber, packed::Byte32), Vec<u32>>,
}

impl FullDeadTransactions {
    pub fn update(&mut self, key: (BlockNumber, packed::Byte32), index: u32) {
        let indices = self.inner.entry(key).or_insert_with(Vec::new);
        indices.push(index);
    }
}

impl PruningTask {
    fn is_safe_pruning(&self) -> bool {
        self.pruning_epoch + 1 < self.current_epoch_ext.number()
    }

    fn is_finished_cycle(&self) -> bool {
        (self.pruning_epoch != 0) && (self.pruning_epoch + 1 == self.current_epoch_ext.number())
    }

    fn pruning_full_dead_transaction(&self) -> Result<(), Error> {
        if self.is_finished_cycle() {
            ckb_logger::info!(
                "recycle pruning_epoch: {} current_epoch: {}",
                self.pruning_epoch,
                self.current_epoch_ext.number()
            );
            self.recycle()?;
            return Ok(());
        }
        ckb_logger::info!("is_safe_pruning: {}", self.is_safe_pruning());
        if self.is_safe_pruning() {
            let epoch_ext = self
                .snapshot
                .get_epoch_index(self.pruning_epoch)
                .and_then(|index| self.snapshot.get_epoch_ext(&index))
                .expect("get_epoch_ext");

            ckb_logger::info!("pruning epoch: {:?}", self.pruning_epoch);

            //skip genesis
            let start = cmp::max(epoch_ext.start_number(), 1);
            let end = epoch_ext.start_number() + epoch_ext.length();
            ckb_logger::info!("pruning number range: {:?}", start..end);
            let full_dead = self.collect_full_dead_transaction(start..end);
            ckb_logger::info!("pruning full_dead count: {}", full_dead.inner.len());
            self.delete_full_dead_transaction(self.pruning_epoch, full_dead)?;
        }
        Ok(())
    }

    fn recycle(&self) -> Result<(), Error> {
        let meta_cf = self.shared.store().cf_handle(COLUMN_META)?;
        let mut wb = WriteBatch::default();
        wb.put_cf(meta_cf, META_PRUNING_EPOCH_KEY, &(0u64).to_be_bytes()[..])
            .map_err(|e| InternalErrorKind::Database.reason(e))?;
        self.shared.store().write_batch(&wb)?;
        Ok(())
    }

    fn delete_full_dead_transaction(
        &self,
        pruning_epoch: EpochNumber,
        txs: FullDeadTransactions,
    ) -> Result<(), Error> {
        let start_time = Instant::now();

        let mut wb = WriteBatch::default();
        let block_body_cf = self.shared.store().cf_handle(COLUMN_BLOCK_BODY)?;
        let pruned_cf = self.shared.store().cf_handle(COLUMN_PRUNED)?;
        let meta_cf = self.shared.store().cf_handle(COLUMN_META)?;

        for (key, indices) in txs.inner {
            let (number, hash) = key;
            for index in indices {
                let key = packed::TransactionKey::new_builder()
                    .block_hash(hash.clone())
                    .index(index.pack())
                    .build();
                wb.delete_cf(block_body_cf, key.as_slice())
                    .map_err(|e| InternalErrorKind::Database.reason(e))?;
            }
            wb.put_cf(pruned_cf, &number.to_be_bytes()[..], &[])
                .map_err(|e| InternalErrorKind::Database.reason(e))?;
            wb.put_cf(pruned_cf, hash.as_slice(), &[])
                .map_err(|e| InternalErrorKind::Database.reason(e))?;
        }
        wb.put_cf(
            meta_cf,
            META_PRUNING_EPOCH_KEY,
            &(pruning_epoch + 1).to_be_bytes()[..],
        )
        .map_err(|e| InternalErrorKind::Database.reason(e))?;
        if !wb.is_empty() {
            self.shared.store().write_batch(&wb)?;
        }

        ckb_logger::info!(
            "delete_full_dead_transaction cost {}",
            start_time.elapsed().as_micros()
        );
        Ok(())
    }

    fn collect_full_dead_transaction(
        &self,
        range: impl IntoIterator<Item = BlockNumber>,
    ) -> FullDeadTransactions {
        let start_time = Instant::now();
        let mut ret = FullDeadTransactions::default();
        for number in range.into_iter() {
            if let Some((tx_hashes, blk_hash)) =
                self.snapshot.get_block_hash(number.into()).map(|hash| {
                    let prefix = hash.as_slice();
                    let tx_hashes = self
                        .snapshot
                        .get_iter(
                            COLUMN_BLOCK_BODY,
                            IteratorMode::From(prefix, IterDirection::Forward),
                        )
                        .take_while(|(key, _)| key.starts_with(prefix))
                        .map(|(key, value)| {
                            let tx_reader = packed::TransactionViewReader::from_slice_should_be_ok(
                                &value.as_ref(),
                            );
                            let tx_hash = tx_reader.hash().to_entity();

                            let key_reader = packed::TransactionKeyReader::from_slice_should_be_ok(
                                &key.as_ref(),
                            );
                            let index: u32 = key_reader.index().unpack();
                            (index, tx_hash)
                        })
                        .collect::<Vec<_>>();
                    (tx_hashes, hash)
                })
            {
                for (index, tx_hash) in tx_hashes {
                    if self.snapshot.get_tx_meta(&tx_hash).is_none() {
                        ret.update((number, blk_hash.clone()), index);
                    }
                }
            }
        }

        ckb_logger::info!(
            "collect_full_dead_transaction cost {}",
            start_time.elapsed().as_micros()
        );
        ret
    }

    fn pruning_detached_block(&self) -> Result<(), Error> {
        self.pruning_marked_block()?;
        if !self.have_pruned {
            self.init_pruning_detached_blocks()?;
        }

        Ok(())
    }

    fn pruning_marked_block(&self) -> Result<(), Error> {
        let hashes = self.collect_marked_blocks();
        ckb_logger::info!("collect_marked_blocks len {}", hashes.len());
        self.pruning_blocks(&hashes)?;
        self.cleanup_pruning_mark(&hashes)?;
        Ok(())
    }

    fn cleanup_pruning_mark(&self, hashes: &[packed::Byte32]) -> Result<(), Error> {
        let mut wb = WriteBatch::default();
        self.shared.store().batch_delete(
            &mut wb,
            COLUMN_PRUNE_MASK,
            hashes.iter().map(|hash| hash.as_slice()),
        )?;
        if !wb.is_empty() {
            self.shared.store().write_batch(&wb)?;
        }
        Ok(())
    }

    fn init_pruning_detached_blocks(&self) -> Result<(), Error> {
        let (pruning, mark) = self.collect_detached_block();
        ckb_logger::info!(
            "init_pruning_detached_blocks pruning: {} mark: {}",
            pruning.len(),
            mark.len()
        );
        self.pruning_blocks(&pruning)?;
        self.mark_pruning(&mark)?;
        Ok(())
    }

    fn mark_pruning(&self, headers: &[HeaderView]) -> Result<(), Error> {
        let mut wb = WriteBatch::default();
        let cf = self.shared.store().cf_handle(COLUMN_PRUNE_MASK)?;
        for header in headers {
            wb.put_cf(
                cf,
                header.hash().as_slice(),
                &header.epoch().number().to_be_bytes()[..],
            )
            .map_err(|e| InternalErrorKind::Database.reason(e))?;
        }
        self.shared.store().write_batch(&wb)?;
        Ok(())
    }

    fn pruning_block_headers(
        &self,
        wb: &mut WriteBatch,
        hashes: &[packed::Byte32],
    ) -> Result<(), Error> {
        self.shared.store().batch_delete(
            wb,
            COLUMN_BLOCK_HEADER,
            hashes.iter().map(|hash| hash.as_slice()),
        )
    }

    fn pruning_block_uncles(
        &self,
        wb: &mut WriteBatch,
        hashes: &[packed::Byte32],
    ) -> Result<(), Error> {
        self.shared.store().batch_delete(
            wb,
            COLUMN_BLOCK_UNCLE,
            hashes.iter().map(|hash| hash.as_slice()),
        )
    }

    fn pruning_block_proposal_ids(
        &self,
        wb: &mut WriteBatch,
        hashes: &[packed::Byte32],
    ) -> Result<(), Error> {
        self.shared.store().batch_delete(
            wb,
            COLUMN_BLOCK_PROPOSAL_IDS,
            hashes.iter().map(|hash| hash.as_slice()),
        )
    }

    fn pruning_block_transactions(&self, hash: &packed::Byte32) -> Result<(), Error> {
        let prefix = hash.as_slice();
        let raw_start_key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(0u32.pack())
            .build();
        let start_key = raw_start_key.as_slice();
        let end_key = self
            .snapshot
            .get_iter(
                COLUMN_BLOCK_BODY,
                IteratorMode::From(start_key, IterDirection::Forward),
            )
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(prefix))
            .last()
            .expect("transactions not empty");
        self.shared
            .store()
            .unsafe_delete_range(COLUMN_BLOCK_BODY, start_key, &end_key)
    }

    fn pruning_blocks(&self, hashes: &[packed::Byte32]) -> Result<(), Error> {
        let start_time = Instant::now();

        let mut wb = WriteBatch::default();
        self.pruning_block_headers(&mut wb, &hashes)?;
        self.pruning_block_uncles(&mut wb, &hashes)?;
        self.pruning_block_proposal_ids(&mut wb, &hashes)?;
        if !wb.is_empty() {
            self.shared.store().write_batch(&wb)?;
        }
        for hash in hashes {
            self.pruning_block_transactions(hash)?;
        }

        ckb_logger::info!(
            "pruning_blocks pruning: {} cost: {}",
            hashes.len(),
            start_time.elapsed().as_micros()
        );
        Ok(())
    }

    fn collect_marked_blocks(&self) -> Vec<packed::Byte32> {
        let start_time = Instant::now();
        let mut pruning = Vec::new();
        for (raw_hash, raw_epoch) in self
            .snapshot
            .get_iter(COLUMN_PRUNE_MASK, IteratorMode::Start)
        {
            let epoch = read_be_u64(&raw_epoch);
            if epoch + 1 < self.current_epoch_ext.number() {
                let hash = packed::Byte32Reader::from_slice_should_be_ok(&raw_hash[..]).to_entity();
                if !self.snapshot.is_main_chain(&hash) {
                    pruning.push(hash);
                }
            }
        }
        ckb_logger::info!(
            "collect_marked_blocks pruning: {} cost: {}",
            pruning.len(),
            start_time.elapsed().as_micros()
        );
        pruning
    }

    fn collect_detached_block(&self) -> (Vec<packed::Byte32>, Vec<HeaderView>) {
        let mut pruning = Vec::new();
        let mut mark = Vec::new();
        for (raw_hash, raw_header) in self
            .snapshot
            .get_iter(COLUMN_BLOCK_HEADER, IteratorMode::Start)
        {
            let hash = packed::Byte32Reader::from_slice_should_be_ok(&raw_hash[..]).to_entity();
            let reader = packed::HeaderViewReader::from_slice_should_be_ok(&raw_header);
            let header = Unpack::<HeaderView>::unpack(&reader);

            if !self.snapshot.is_main_chain(&hash) {
                if header.epoch().number() + 1 < self.current_epoch_ext.number() {
                    pruning.push(hash);
                } else {
                    mark.push(header)
                }
            }
        }
        (pruning, mark)
    }
}

fn read_be_u64(input: &[u8]) -> u64 {
    let (int_bytes, _rest) = input.split_at(mem::size_of::<u64>());
    u64::from_be_bytes(int_bytes.try_into().expect("read u64 from be"))
}

impl PruneService {
    pub fn new(shared: Shared) -> PruneService {
        PruneService { shared }
    }

    fn new_task(&self) -> PruningTask {
        let snapshot = Arc::clone(&self.shared.snapshot());
        let pruning_epoch = snapshot.get_pruning_epoch();
        let current_epoch_ext = snapshot.epoch_ext().clone();

        PruningTask {
            have_pruned: pruning_epoch.is_some(),
            pruning_epoch: pruning_epoch.unwrap_or(0),
            current_epoch_ext,
            shared: self.shared.clone(),
            snapshot,
        }
    }

    pub fn start(self) -> PruneController {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(service::SIGNAL_CHANNEL_SIZE);
        let thread = thread::Builder::new()
            .spawn(move || loop {
                match signal_receiver.recv_timeout(Duration::from_secs(100)) {
                    Err(_) => {
                        let task = self.new_task();
                        if let Err(e) = task.pruning_detached_block() {
                            ckb_logger::error!("pruning_detached_block error {}", e);
                        }
                        if let Err(e) = task.pruning_full_dead_transaction() {
                            ckb_logger::error!("pruning_full_dead_transaction error {}", e);
                        }
                    }
                    Ok(_) => {
                        ckb_logger::info!("PruneService closing");
                        break;
                    }
                }
            })
            .expect("Start ChainService failed");

        let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), thread);
        PruneController { stop }
    }
}
