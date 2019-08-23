use crate::block_assembler::{BlockAssembler, TemplateCache};
use crate::component::commit_txs_scanner::CommitTxsScanner;
use crate::error::BlockAssemblerError;
use crate::pool::TxPool;
use crate::process::util::MaybeAcquired;
use crate::service::BlockTemplateResult;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    BlockNumber as JsonBlockNumber, BlockTemplate, Cycle as JsonCycle,
    EpochNumber as JsonEpochNumber, Timestamp as JsonTimestamp, Unsigned, Version as JsonVersion,
};
use ckb_logger::info;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{resolve_transaction, OverlayCellProvider, TransactionsProvider},
        ScriptHashType, Version,
    },
    packed::{self, Script},
    prelude::*,
};
use faketime::unix_time_as_millis;
use futures::future::Future;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::{cmp, iter};
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct BlockTemplateProcess {
    pub tx_pool: MaybeAcquired<TxPool>,
    pub block_assembler: MaybeAcquired<BlockAssembler>,
    pub bytes_limit: Option<u64>,
    pub proposals_limit: Option<u64>,
    pub max_version: Option<Version>,
}

impl BlockTemplateProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        block_assembler: Lock<BlockAssembler>,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> BlockTemplateProcess {
        BlockTemplateProcess {
            tx_pool: MaybeAcquired::NotYet(tx_pool),
            block_assembler: MaybeAcquired::NotYet(block_assembler),
            bytes_limit,
            proposals_limit,
            max_version,
        }
    }
}

impl Future for BlockTemplateProcess {
    type Item = BlockTemplateResult;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let tx_pool_acquired = self.tx_pool.poll();
        let block_assembler_acquired = self.block_assembler.poll();

        if tx_pool_acquired && block_assembler_acquired {
            let mut tx_pool = self.tx_pool.take();
            let mut block_assembler = self.block_assembler.take();

            let builder = BlockTemplateBuilder {
                tx_pool: &mut tx_pool,
                block_assembler: &mut block_assembler,
            };
            Ok(Async::Ready(builder.build(
                self.bytes_limit,
                self.proposals_limit,
                self.max_version,
            )))
        } else {
            Ok(Async::NotReady)
        }
    }
}

struct BlockTemplateBuilder<'a> {
    tx_pool: &'a mut TxPool,
    block_assembler: &'a mut BlockAssembler,
}

impl<'a> BlockTemplateBuilder<'a> {
    fn build(
        self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> BlockTemplateResult {
        let snapshot = self.tx_pool.snapshot();
        let last_txs_updated_at = self.tx_pool.get_last_txs_updated_at();
        let consensus = snapshot.consensus();

        let cycles_limit = consensus.max_block_cycles();
        let (bytes_limit, proposals_limit, version) = self.block_assembler.transform_params(
            consensus,
            bytes_limit,
            proposals_limit,
            max_version,
        );
        let uncles_count_limit = consensus.max_uncles_num() as u32;

        let last_uncles_updated_at = self.block_assembler.load_last_uncles_updated_at();

        // try get cache
        let tip_header = snapshot.get_tip_header().expect("get tip header");
        let tip_hash = tip_header.hash();
        let candidate_number = tip_header.number() + 1;
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);
        if let Some(template_cache) = self.block_assembler.template_caches.get(&(
            tip_header.hash().unpack(),
            cycles_limit,
            bytes_limit,
            version,
        )) {
            // check template cache outdate time
            if !template_cache.is_outdate(current_time) {
                let mut template = template_cache.template.clone();
                template.current_time = JsonTimestamp(current_time);
                return Ok(template);
            }

            if !template_cache.is_modified(last_uncles_updated_at, last_txs_updated_at) {
                let mut template = template_cache.template.clone();
                template.current_time = JsonTimestamp(current_time);
                return Ok(template);
            }
        }

        let last_epoch = snapshot.get_current_epoch_ext().expect("current epoch ext");
        let next_epoch_ext = snapshot.next_epoch_ext(consensus, &last_epoch, &tip_header);
        let current_epoch = next_epoch_ext.unwrap_or(last_epoch);
        let uncles =
            self.block_assembler
                .prepare_uncles(&snapshot, candidate_number, &current_epoch);

        let cellbase_lock_args = self
            .block_assembler
            .config
            .args
            .clone()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<packed::Bytes>>();

        let hash_type: ScriptHashType = self.block_assembler.config.hash_type.clone().into();
        let cellbase_lock = Script::new_builder()
            .args(cellbase_lock_args.pack())
            .code_hash(self.block_assembler.config.code_hash.pack())
            .hash_type(hash_type.pack())
            .build();

        let cellbase =
            self.block_assembler
                .build_cellbase(&snapshot, &tip_header, cellbase_lock)?;

        let (proposals, entries, last_txs_updated_at) = {
            let proposals = self.tx_pool.get_proposals(proposals_limit as usize);
            let txs_size_limit = self.block_assembler.calculate_txs_size_limit(
                bytes_limit,
                cellbase.data(),
                &uncles,
                &proposals,
            )?;

            let (entries, size, cycles) = CommitTxsScanner::new(self.tx_pool.proposed())
                .txs_to_commit(txs_size_limit, cycles_limit);
            if !entries.is_empty() {
                info!(
                    "[get_block_template] candidate txs count: {}, size: {}/{}, cycles:{}/{}",
                    entries.len(),
                    size,
                    txs_size_limit,
                    cycles,
                    cycles_limit
                );
            }
            (proposals, entries, last_txs_updated_at)
        };

        let mut txs = iter::once(&cellbase).chain(entries.iter().map(|entry| &entry.transaction));

        let mut seen_inputs = HashSet::new();
        let transactions_provider = TransactionsProvider::new(txs.clone());
        let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, snapshot);

        let rtxs = txs
            .try_fold(vec![], |mut rtxs, tx| {
                match resolve_transaction(tx, &mut seen_inputs, &overlay_cell_provider, snapshot) {
                    Ok(rtx) => {
                        rtxs.push(rtx);
                        Ok(rtxs)
                    }
                    Err(e) => Err(e),
                }
            })
            .map_err(|_| BlockAssemblerError::InvalidInput)?;
        // Generate DAO fields here
        let dao = DaoCalculator::new(consensus, snapshot).dao_field(&rtxs, &tip_header)?;

        // Should recalculate current time after create cellbase (create cellbase may spend a lot of time)
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);
        let template = BlockTemplate {
            version: JsonVersion(version),
            difficulty: current_epoch.difficulty().clone(),
            current_time: JsonTimestamp(current_time),
            number: JsonBlockNumber(candidate_number),
            epoch: JsonEpochNumber(current_epoch.number()),
            parent_hash: tip_hash.unpack(),
            cycles_limit: JsonCycle(cycles_limit),
            bytes_limit: Unsigned(bytes_limit),
            uncles_count_limit: Unsigned(uncles_count_limit.into()),
            uncles: uncles
                .into_iter()
                .map(BlockAssembler::transform_uncle)
                .collect(),
            transactions: entries
                .iter()
                .map(|entry| BlockAssembler::transform_tx(entry, false, None))
                .collect(),
            proposals: proposals.into_iter().map(Into::into).collect(),
            cellbase: BlockAssembler::transform_cellbase(&cellbase, None),
            work_id: Unsigned(self.block_assembler.work_id.fetch_add(1, Ordering::SeqCst) as u64),
            dao: dao.into(),
        };

        self.block_assembler.template_caches.insert(
            (tip_hash.unpack(), cycles_limit, bytes_limit, version),
            TemplateCache {
                time: current_time,
                uncles_updated_at: last_uncles_updated_at,
                txs_updated_at: last_txs_updated_at,
                template: template.clone(),
            },
        );

        Ok(template)
    }
}
