//! Tx-pool message dispatching.

use crate::component::pool_map::Status;
use crate::service::{
    AsyncRequest, BlockTemplateArgs, BlockTemplateResult, FeeEstimatesResult,
    FetchTxsWithCyclesResult, GetTransactionWithStatusResult, GetTxStatusResult, Message, Notify,
    PipelineTxLocation, ResolvedTxLocation, SubmitTxResult, SyncRequest, TestAcceptTxResult,
    TxPoolService, map_pool_status, respond,
};
use crate::tx_source::TxSource;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        EstimateMode, TransactionView, UncleBlockView,
        cell::CellStatus,
        tx_pool::{
            PoolTxDetailInfo, TransactionWithStatus, TxPoolEntryInfo, TxPoolIds, TxPoolInfo,
            TxStatus,
        },
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(feature = "internal")]
use crate::{component::entry::TxEntry, process::PlugTarget};

pub(crate) async fn process(mut service: TxPoolService, message: Message) {
    match message {
        Message::GetTxPoolInfo(req) => service.handle_get_tx_pool_info(req).await,
        Message::GetLiveCell(req) => service.handle_get_live_cell(req).await,
        Message::BlockTemplate(req) => service.handle_block_template(req).await,
        Message::SubmitLocalTx(req) => service.handle_submit_local_tx(req).await,
        Message::SubmitLocalTestTx(req) => service.handle_submit_local_test_tx(req).await,
        Message::RemoveLocalTx(req) => service.handle_remove_local_tx(req).await,
        Message::TestAcceptTx(req) => service.handle_test_accept_tx(req).await,
        Message::SubmitRemoteTx(req) => service.handle_submit_remote_tx(req).await,
        Message::NotifyTxs(req) => service.handle_notify_txs(req).await,
        Message::FreshProposalsFilter(req) => service.handle_fresh_proposals_filter(req).await,
        Message::GetTxStatus(req) => service.handle_get_tx_status(req).await,
        Message::GetTransactionWithStatus(req) => {
            service.handle_get_transaction_with_status(req).await;
        }
        Message::FetchTxs(req) => service.handle_fetch_txs(req).await,
        Message::FetchTxsWithCycles(req) => service.handle_fetch_txs_with_cycles(req).await,
        Message::NewUncle(req) => service.handle_new_uncle(req).await,
        Message::ClearPool(req) => service.handle_clear_pool(req).await,
        Message::ClearPipeline(req) => service.handle_clear_pipeline(req).await,
        Message::GetPoolTxDetails(req) => service.handle_get_pool_tx_details(req).await,
        Message::GetAllEntryInfo(req) => service.handle_get_all_entry_info(req).await,
        Message::GetAllIds(req) => service.handle_get_all_ids(req).await,
        Message::SavePool(req) => service.handle_save_pool(req).await,
        Message::UpdateIBDState(req) => service.handle_update_ibd_state(req).await,
        Message::EstimateFeeRate(req) => service.handle_estimate_fee_rate(req).await,
        #[cfg(feature = "internal")]
        Message::PlugEntry(req) => service.handle_plug_entry(req).await,
        #[cfg(feature = "internal")]
        Message::PackageTxs(req) => service.handle_package_txs(req).await,
        Message::GetTotalRecentRejectNum(req) => {
            service.handle_get_total_recent_reject_num(req).await;
        }
    }
}

impl TxPoolService {
    async fn handle_get_tx_pool_info(&self, req: SyncRequest<(), TxPoolInfo>) {
        let SyncRequest { responder, .. } = req;
        let info = self.info().await;
        respond(responder, info, "get_tx_pool_info");
    }

    async fn handle_get_live_cell(&self, req: SyncRequest<(OutPoint, bool), CellStatus>) {
        let SyncRequest {
            responder,
            arguments: (out_point, with_data),
        } = req;
        let live_cell_status = self.get_live_cell(out_point, with_data).await;
        respond(responder, live_cell_status, "get_live_cell");
    }

    async fn handle_block_template(
        &self,
        req: SyncRequest<BlockTemplateArgs, BlockTemplateResult>,
    ) {
        let SyncRequest { responder, .. } = req;
        let block_template_result = self.get_block_template().await;
        respond(responder, block_template_result, "block_template_result");
    }

    async fn handle_submit_local_tx(&self, req: SyncRequest<TransactionView, SubmitTxResult>) {
        let SyncRequest {
            responder,
            arguments: tx,
        } = req;
        let result = self.process_tx(tx, TxSource::local()).await.map(|_| ());
        respond(responder, result, "submit_local_tx");
    }

    async fn handle_submit_local_test_tx(&self, req: SyncRequest<TransactionView, SubmitTxResult>) {
        let SyncRequest {
            responder,
            arguments: tx,
        } = req;
        let result = async {
            self.check_tx_basic_validity(&tx, TxSource::local()).await?;
            self.classify_and_enqueue_tx_spawn(tx, TxSource::local())
                .await
                .map(|_| ())
        }
        .await;
        respond(responder, result, "submit_local_test_tx");
    }

    async fn handle_remove_local_tx(&self, req: SyncRequest<Byte32, bool>) {
        let SyncRequest {
            responder,
            arguments: tx_hash,
        } = req;
        let result = self.remove_tx(tx_hash).await;
        respond(responder, result, "remove_tx");
    }

    async fn handle_test_accept_tx(&self, req: SyncRequest<TransactionView, TestAcceptTxResult>) {
        let SyncRequest {
            responder,
            arguments: tx,
        } = req;
        let result = self.test_accept_tx(tx).await;
        respond(responder, result.map(|r| r.into()), "test_accept_tx");
    }

    async fn handle_submit_remote_tx(&self, req: SyncRequest<(TransactionView, TxSource), ()>) {
        let SyncRequest {
            responder,
            arguments: (tx, source),
        } = req;
        let _result = self.submit_remote_tx(tx, source).await;
        respond(responder, (), "submit_remote_tx");
    }

    async fn handle_notify_txs(&self, req: Notify<Vec<TransactionView>>) {
        let Notify { arguments: txs } = req;
        for tx in txs {
            let _ret = self.notify_tx(tx).await;
        }
    }

    async fn handle_fresh_proposals_filter(
        &self,
        req: AsyncRequest<Vec<ProposalShortId>, Vec<ProposalShortId>>,
    ) {
        let AsyncRequest {
            responder,
            arguments: proposals,
        } = req;
        let new_proposals = self.exclude_existing_proposal(proposals).await;
        respond(responder, new_proposals, "fresh_proposals_filter");
    }

    /// Look up a transaction in the main pool or in the in-flight pipeline.
    async fn resolve_tx_location(&self, hash: &Byte32) -> ResolvedTxLocation {
        let id = ProposalShortId::from_tx_hash(hash);
        let pool_entry = {
            let tx_pool = self.tx_pool.read().await;
            tx_pool.pool_map.get_by_id(&id).map(|entry| {
                let status = entry.status;
                let entry = entry.inner.clone();
                (status, entry)
            })
        };
        if let Some((status, entry)) = pool_entry {
            return ResolvedTxLocation::Pool { status, entry };
        }
        if let Some(location) = self.find_tx_in_pipeline(&id).await {
            return ResolvedTxLocation::Pipeline(location);
        }
        ResolvedTxLocation::NotFound
    }

    async fn handle_get_tx_status(&self, req: SyncRequest<Byte32, GetTxStatusResult>) {
        let SyncRequest {
            responder,
            arguments: hash,
        } = req;
        let ret = match self.resolve_tx_location(&hash).await {
            ResolvedTxLocation::Pool { status, entry } => {
                Ok((map_pool_status(status), Some(entry.cycles)))
            }
            ResolvedTxLocation::Pipeline(_) => Ok((TxStatus::Pending, None)),
            ResolvedTxLocation::NotFound => {
                self.lookup_recent_reject(
                    &hash,
                    |record| (TxStatus::Rejected(record), None),
                    || (TxStatus::Unknown, None),
                )
                .await
            }
        };
        respond(responder, ret, "get_tx_status");
    }

    async fn handle_get_transaction_with_status(
        &self,
        req: SyncRequest<Byte32, GetTransactionWithStatusResult>,
    ) {
        let SyncRequest {
            responder,
            arguments: hash,
        } = req;
        let ret = match self.resolve_tx_location(&hash).await {
            ResolvedTxLocation::Pool { status, entry } => {
                let min_replace_fee = {
                    let tx_pool = self.tx_pool.read().await;
                    if status == Status::Proposed {
                        None
                    } else {
                        tx_pool.min_replace_fee(&entry)
                    }
                };
                Ok(TransactionWithStatus::with_status(
                    Some(entry.transaction().clone()),
                    entry.cycles,
                    entry.timestamp,
                    map_pool_status(status),
                    Some(entry.fee),
                    min_replace_fee,
                ))
            }
            ResolvedTxLocation::Pipeline(location) => {
                let (tx, tx_status, cycles, fee) = match location {
                    PipelineTxLocation::PreChecking { tx } => (tx, TxStatus::Pending, None, None),
                    PipelineTxLocation::Ordered { tx } => (tx, TxStatus::Pending, None, None),
                    PipelineTxLocation::Verifying { tx, fee, status } => {
                        let tx_status = if status == Status::Proposed {
                            TxStatus::Proposed
                        } else {
                            TxStatus::Pending
                        };
                        (tx, tx_status, None, Some(fee))
                    }
                    PipelineTxLocation::Orphan { tx, cycle } => {
                        (tx, TxStatus::Pending, Some(cycle), None)
                    }
                };
                Ok(TransactionWithStatus {
                    transaction: Some(tx),
                    tx_status,
                    cycles,
                    fee,
                    min_replace_fee: None,
                    time_added_to_pool: None,
                })
            }
            ResolvedTxLocation::NotFound => {
                self.lookup_recent_reject(
                    &hash,
                    TransactionWithStatus::with_rejected,
                    TransactionWithStatus::with_unknown,
                )
                .await
            }
        };
        respond(responder, ret, "get_transaction_with_status");
    }

    async fn handle_fetch_txs(
        &self,
        req: AsyncRequest<HashSet<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>,
    ) {
        let AsyncRequest {
            responder,
            arguments: short_ids,
        } = req;
        let txs_map = self.get_tx_for_compact_block(short_ids).await;
        respond(responder, txs_map, "fetch_txs");
    }

    async fn handle_fetch_txs_with_cycles(
        &self,
        req: AsyncRequest<HashSet<ProposalShortId>, FetchTxsWithCyclesResult>,
    ) {
        let AsyncRequest {
            responder,
            arguments: short_ids,
        } = req;
        let tx_pool = self.tx_pool.read().await;
        let txs = short_ids
            .into_iter()
            .filter_map(|short_id| {
                tx_pool
                    .get_tx_with_cycles(&short_id)
                    .map(|(tx, cycles)| (short_id, (tx, cycles)))
            })
            .collect();
        respond(responder, txs, "fetch_txs_with_cycles");
    }

    async fn handle_new_uncle(&self, req: Notify<UncleBlockView>) {
        let Notify { arguments: uncle } = req;
        self.receive_candidate_uncle(uncle).await;
    }

    async fn handle_clear_pool(&mut self, req: SyncRequest<Arc<Snapshot>, ()>) {
        let SyncRequest {
            responder,
            arguments: new_snapshot,
        } = req;
        self.clear_pool(new_snapshot).await;
        respond(responder, (), "clear_pool");
    }

    async fn handle_clear_pipeline(&self, req: SyncRequest<(), ()>) {
        let SyncRequest { responder, .. } = req;
        self.clear_pipeline_queues().await;
        respond(responder, (), "clear_pipeline");
    }

    async fn handle_get_pool_tx_details(&self, req: SyncRequest<Byte32, PoolTxDetailInfo>) {
        let SyncRequest {
            responder,
            arguments: tx_hash,
        } = req;
        let tx_pool = self.tx_pool.read().await;
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        let tx_details = tx_pool
            .get_tx_detail(&id)
            .unwrap_or(PoolTxDetailInfo::with_unknown());
        respond(responder, tx_details, "get_pool_tx_details");
    }

    async fn handle_get_all_entry_info(&self, req: SyncRequest<(), TxPoolEntryInfo>) {
        let SyncRequest { responder, .. } = req;
        let tx_pool = self.tx_pool.read().await;
        let info = tx_pool.get_all_entry_info();
        respond(responder, info, "get_all_entry_info");
    }

    async fn handle_get_all_ids(&self, req: SyncRequest<(), TxPoolIds>) {
        let SyncRequest { responder, .. } = req;
        let tx_pool = self.tx_pool.read().await;
        let ids = tx_pool.get_ids();
        respond(responder, ids, "get_ids");
    }

    async fn handle_save_pool(&self, req: SyncRequest<(), ()>) {
        let SyncRequest { responder, .. } = req;
        self.save_pool().await;
        respond(responder, (), "save_pool");
    }

    async fn handle_update_ibd_state(&self, req: SyncRequest<bool, ()>) {
        let SyncRequest {
            responder,
            arguments: in_ibd,
        } = req;
        self.update_ibd_state(in_ibd).await;
        respond(responder, (), "update_ibd_state");
    }

    async fn handle_estimate_fee_rate(
        &self,
        req: SyncRequest<(EstimateMode, bool), FeeEstimatesResult>,
    ) {
        let SyncRequest {
            responder,
            arguments: (estimate_mode, enable_fallback),
        } = req;
        let fee_estimates_result = self.estimate_fee_rate(estimate_mode, enable_fallback).await;
        respond(responder, fee_estimates_result, "fee_estimates_result");
    }

    #[cfg(feature = "internal")]
    async fn handle_plug_entry(&self, req: SyncRequest<(Vec<TxEntry>, PlugTarget), ()>) {
        let SyncRequest {
            responder,
            arguments: (entries, target),
        } = req;
        self.plug_entry(entries, target).await;
        respond(responder, (), "plug_entry");
    }

    #[cfg(feature = "internal")]
    async fn handle_package_txs(&self, req: SyncRequest<Option<u64>, Vec<TxEntry>>) {
        let SyncRequest {
            responder,
            arguments: bytes_limit,
        } = req;
        let max_block_cycles = self.consensus.max_block_cycles();
        let max_block_bytes = self.consensus.max_block_bytes();
        let tx_pool = self.tx_pool.read().await;
        let (txs, _size, _cycles) = tx_pool.package_txs(
            max_block_cycles,
            bytes_limit.unwrap_or(max_block_bytes) as usize,
        );
        respond(responder, txs, "package_txs");
    }

    async fn handle_get_total_recent_reject_num(&self, req: SyncRequest<(), Option<u64>>) {
        let SyncRequest { responder, .. } = req;
        let total_recent_reject_num = self.get_total_recent_reject_num();
        respond(
            responder,
            total_recent_reject_num,
            "total_recent_reject_num",
        );
    }
}
