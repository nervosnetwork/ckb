//! Exhaustive conversion from controller messages to unified-authority APIs.

use super::builder::RetainedIngressBatch;
use crate::{
    authority::{
        query::{
            AuthorityPoolSummary, AuthorityTransactionLookup, AuthorityTransactionStatusLookup,
            PublicPoolStatus,
        },
        service::{
            AuthorityDerivedError, AuthorityGenerationInvalidity, AuthorityPersistenceError,
            AuthorityService, AuthorityServiceError,
        },
    },
    service::{
        AsyncRequest, Message, Notify, OneshotSender, RemoteTxSubmission, Request, SyncRequest,
        respond,
    },
};
use ckb_error::{AnyError, OtherError};
use ckb_types::{
    core::tx_pool::{
        PoolTxDetailInfo, TRANSACTION_SIZE_LIMIT, TransactionWithStatus, TxPoolInfo, TxStatus,
    },
    packed::Byte32,
};

/// Process one message. Only a structural authority contradiction is returned
/// to the generation owner; legal rejection, pressure, cancellation and
/// external derived-service errors are consumed at this compatibility edge.
pub(crate) async fn process(
    service: AuthorityService,
    message: Message,
) -> Result<(), AuthorityGenerationInvalidity> {
    match message {
        Message::GetTxPoolInfo(request) => {
            let Request { responder, .. } = request;
            respond_outer(
                responder,
                service
                    .pool_summary()
                    .await
                    .and_then(|summary| pool_info(&service, summary)),
                "get_tx_pool_info",
            )
        }
        Message::GetLiveCell(request) => {
            let Request {
                responder,
                arguments: (out_point, with_data),
            } = request;
            respond(
                responder,
                service.live_cell_receipt(out_point).resolve(with_data),
                "get_live_cell",
            );
            Ok(())
        }
        Message::BlockTemplate(request) => {
            let Request { responder, .. } = request;
            respond_nested_authority(responder, service.block_template().await, "block_template")
        }
        Message::SubmitLocalTx(request) | Message::SubmitLocalTestTx(request) => {
            let Request {
                responder,
                arguments: transaction,
            } = request;
            respond_outer(
                responder,
                service
                    .submit_local(transaction)
                    .await
                    .map(|result| result.map(|_| ())),
                "submit_local_tx",
            )
        }
        Message::RemoveLocalTx(request) => {
            let Request {
                responder,
                arguments: hash,
            } = request;
            respond_outer(
                responder,
                service.remove_local(&hash).await,
                "remove_local_tx",
            )
        }
        Message::TestAcceptTx(request) => {
            let Request {
                responder,
                arguments: transaction,
            } = request;
            respond_outer(
                responder,
                service.test_accept(transaction).await,
                "test_accept_tx",
            )
        }
        Message::SubmitRemoteTx(request) => {
            let AsyncRequest {
                responder,
                arguments:
                    RemoteTxSubmission {
                        transaction,
                        declared_cycles,
                        peer,
                    },
            } = request;
            let result = service
                .submit_remote(transaction, declared_cycles, peer)
                .await;
            respond_outer(responder, result, "submit_remote_tx")
        }
        Message::NotifyTxs(Notify { arguments }) => {
            match service
                .submit_proposal_batch(arguments.into_transactions())
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => settle_service_error(error),
            }
        }
        Message::FreshProposalsFilter(request) => {
            let AsyncRequest {
                responder,
                arguments,
            } = request;
            respond_outer(
                responder,
                service.filter_fresh_proposals(arguments.into_vec()),
                "fresh_proposals_filter",
            )
        }
        Message::FetchTxs(request) => {
            let AsyncRequest {
                responder,
                arguments,
            } = request;
            respond_outer(
                responder,
                service.compact_transactions(arguments.into_vec()),
                "fetch_txs",
            )
        }
        Message::FetchTxsWithCycles(request) => {
            let AsyncRequest {
                responder,
                arguments,
            } = request;
            let result = service.accepted_with_cycles(arguments.into_vec());
            respond_outer(responder, result, "fetch_txs_with_cycles")
        }
        Message::GetTxStatus(request) => handle_get_tx_status(&service, request),
        Message::GetTransactionWithStatus(request) => {
            handle_get_transaction_with_status(&service, request)
        }
        Message::NewUncle(Notify { arguments }) => {
            service.receive_candidate_uncle(arguments);
            Ok(())
        }
        Message::GetPoolTxDetails(request) => {
            let Request {
                responder,
                arguments,
            } = request;
            respond_outer(
                responder,
                service
                    .pool_detail(&arguments)
                    .await
                    .map(|detail| detail.unwrap_or_else(PoolTxDetailInfo::with_unknown)),
                "get_pool_tx_details",
            )
        }
        Message::GetAllEntryInfo(request) => {
            let Request { responder, .. } = request;
            respond_outer(
                responder,
                service.all_entry_info().await,
                "get_all_entry_info",
            )
        }
        Message::GetAllIds(request) => {
            let Request { responder, .. } = request;
            respond_outer(responder, service.pool_ids().await, "get_all_ids")
        }
        Message::SavePool(request) => {
            let Request { responder, .. } = request;
            match service.save_pool().await {
                Ok(()) => {
                    respond(responder, (), "save_pool");
                    Ok(())
                }
                Err(AuthorityPersistenceError::Snapshot(error)) => {
                    drop(responder);
                    settle_service_error(error)
                }
                Err(error) => {
                    ckb_logger::error!("explicit tx-pool save failed: {error:?}");
                    respond(responder, (), "save_pool");
                    Ok(())
                }
            }
        }
        Message::UpdateIBDState(request) => {
            let Request {
                responder,
                arguments,
            } = request;
            service.update_ibd_state(arguments);
            respond(responder, (), "update_ibd_state");
            Ok(())
        }
        Message::EstimateFeeRate(request) => {
            let Request {
                responder,
                arguments: (mode, fallback),
            } = request;
            respond_derived(
                responder,
                service.estimate_fee_rate(mode, fallback).await,
                "estimate_fee_rate",
            )
        }
        Message::GetTotalRecentRejectNum(request) => {
            let Request { responder, .. } = request;
            respond(
                responder,
                service.total_recent_reject_num(),
                "get_total_recent_reject_num",
            );
            Ok(())
        }
        #[cfg(feature = "internal")]
        Message::PlugEntry(request) => {
            let Request {
                responder,
                arguments: (entries, target),
            } = request;
            respond(
                responder,
                service.plug_entry(entries, target).await,
                "plug_entry",
            );
            Ok(())
        }
        #[cfg(feature = "internal")]
        Message::PackageTxs(request) => {
            let Request {
                responder,
                arguments,
            } = request;
            respond_outer(
                responder,
                service.package_transactions(arguments),
                "package_txs",
            )
        }
    }
}

/// Process one dispatcher-owned homogeneous prefix. The batch contains only
/// payloads already removed from the bounded controller channel; authority
/// admission and every responder still settle exactly once in canonical
/// channel order.
pub(super) async fn process_retained_ingress_batch(
    service: AuthorityService,
    batch: RetainedIngressBatch,
) -> Result<(), AuthorityGenerationInvalidity> {
    match batch {
        RetainedIngressBatch::Remote {
            peer,
            submissions,
            responders,
            ..
        } => {
            let (completed, error) = service
                .submit_remote_batch(peer, submissions)
                .await
                .into_checked_parts(responders.len());
            settle_remote_responder_prefix(responders, completed, error)
        }
        RetainedIngressBatch::Proposal { transactions, .. } => {
            match service.submit_proposal_batch(transactions).await {
                Ok(()) => Ok(()),
                Err(error) => settle_service_error(error),
            }
        }
    }
}

fn settle_remote_responder_prefix(
    responders: Vec<tokio::sync::oneshot::Sender<()>>,
    completed: usize,
    error: Option<AuthorityServiceError>,
) -> Result<(), AuthorityGenerationInvalidity> {
    let expected = responders.len();
    let mut responders = responders.into_iter();
    for responder in responders.by_ref().take(completed.min(expected)) {
        respond(responder, (), "submit_remote_tx");
    }
    // Dropping the suffix is the negative acknowledgement consumed by the
    // relayer's move-only request futures. It releases only known marks whose
    // authority items did not belong to the committed canonical prefix.
    drop(responders);
    error.map_or(Ok(()), settle_service_error)
}

fn pool_info(
    service: &AuthorityService,
    summary: AuthorityPoolSummary,
) -> Result<TxPoolInfo, AuthorityServiceError> {
    let max_tx_pool_size = u64::try_from(service.config().max_tx_pool_size)
        .map_err(|_| AuthorityServiceError::ResourceUnavailable)?;
    Ok(TxPoolInfo {
        tip_hash: summary.tip_hash,
        tip_number: summary.tip_number,
        pending_size: summary.pending_size,
        proposed_size: summary.proposed_size,
        orphan_size: summary.orphan_size,
        total_tx_size: summary.total_tx_size,
        total_tx_cycles: summary.total_tx_cycles,
        min_fee_rate: service.config().min_fee_rate,
        min_rbf_rate: service.config().min_rbf_rate,
        last_txs_updated_at: summary.last_txs_updated_at,
        tx_size_limit: TRANSACTION_SIZE_LIMIT,
        max_tx_pool_size,
        verify_queue_size: summary.verify_queue_size,
    })
}

fn handle_get_tx_status(
    service: &AuthorityService,
    request: SyncRequest<Byte32, crate::service::GetTxStatusResult>,
) -> Result<(), AuthorityGenerationInvalidity> {
    let Request {
        responder,
        arguments: hash,
    } = request;
    match service.transaction_status_lookup(&hash) {
        AuthorityTransactionStatusLookup::Live(transaction) => {
            respond(
                responder,
                Ok((public_status(transaction.status), transaction.cycles)),
                "get_tx_status",
            );
            Ok(())
        }
        AuthorityTransactionStatusLookup::RecentRejectFallback => {
            let result = service.recent_reject_record(&hash).map(|record| {
                record.map_or((TxStatus::Unknown, None), |record| {
                    (TxStatus::Rejected(record), None)
                })
            });
            respond_derived(responder, result, "get_tx_status")
        }
    }
}

fn handle_get_transaction_with_status(
    service: &AuthorityService,
    request: SyncRequest<Byte32, crate::service::GetTransactionWithStatusResult>,
) -> Result<(), AuthorityGenerationInvalidity> {
    let Request {
        responder,
        arguments: hash,
    } = request;
    match service.transaction_lookup(&hash) {
        Ok(AuthorityTransactionLookup::Live(transaction)) => {
            respond(
                responder,
                Ok(TransactionWithStatus {
                    transaction: Some(transaction.transaction.as_ref().clone()),
                    tx_status: public_status(transaction.status),
                    cycles: transaction.cycles,
                    fee: transaction.fee,
                    min_replace_fee: transaction.min_replace_fee,
                    time_added_to_pool: transaction.accepted_at,
                }),
                "get_transaction_with_status",
            );
            Ok(())
        }
        Ok(AuthorityTransactionLookup::RecentRejectFallback) => {
            let result = service.recent_reject_record(&hash).map(|record| {
                record.map_or_else(
                    TransactionWithStatus::with_unknown,
                    TransactionWithStatus::with_rejected,
                )
            });
            respond_derived(responder, result, "get_transaction_with_status")
        }
        Err(error) => {
            drop(responder);
            settle_service_error(error)
        }
    }
}

fn public_status(status: PublicPoolStatus) -> TxStatus {
    match status {
        PublicPoolStatus::Pending => TxStatus::Pending,
        PublicPoolStatus::Proposed => TxStatus::Proposed,
    }
}

fn respond_outer<R, S>(
    responder: S,
    result: Result<R, AuthorityServiceError>,
    message: &'static str,
) -> Result<(), AuthorityGenerationInvalidity>
where
    R: std::fmt::Debug,
    S: OneshotSender<R>,
{
    match result {
        Ok(value) => {
            respond(responder, value, message);
            Ok(())
        }
        Err(error) => {
            drop(responder);
            settle_service_error(error)
        }
    }
}

fn respond_nested_authority<R, S>(
    responder: S,
    result: Result<R, AuthorityServiceError>,
    message: &'static str,
) -> Result<(), AuthorityGenerationInvalidity>
where
    R: std::fmt::Debug,
    S: OneshotSender<Result<R, AnyError>>,
{
    match result {
        Ok(value) => {
            respond(responder, Ok(value), message);
            Ok(())
        }
        Err(error) => {
            respond(responder, Err(authority_error_as_any(&error)), message);
            settle_service_error(error)
        }
    }
}

fn respond_derived<R, S>(
    responder: S,
    result: Result<R, AuthorityDerivedError>,
    message: &'static str,
) -> Result<(), AuthorityGenerationInvalidity>
where
    R: std::fmt::Debug,
    S: OneshotSender<Result<R, AnyError>>,
{
    match result {
        Ok(value) => {
            respond(responder, Ok(value), message);
            Ok(())
        }
        Err(AuthorityDerivedError::External(error)) => {
            respond(responder, Err(error), message);
            Ok(())
        }
        Err(AuthorityDerivedError::Authority(error)) => {
            respond(responder, Err(authority_error_as_any(&error)), message);
            settle_service_error(error)
        }
    }
}

fn settle_service_error(error: AuthorityServiceError) -> Result<(), AuthorityGenerationInvalidity> {
    AuthorityService::settle_operation_error(error)
}

fn authority_error_as_any(error: &AuthorityServiceError) -> AnyError {
    OtherError::new(format!("tx-pool authority service failed: {error:?}")).into()
}

#[cfg(test)]
#[path = "tests/dispatch.rs"]
mod tests;
