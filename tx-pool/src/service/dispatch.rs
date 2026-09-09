//! Direct conversion between the public protocol and pool operations.
use crate::{
    authority::service::{Error, Pool},
    service::{
        Message, Notify, OneshotSender, RemoteTxBatchOutcome, RemoteTxSubmission, Request, respond,
    },
};
use ckb_error::AnyError;
use std::sync::Arc;

pub(crate) async fn process(pool: Arc<Pool>, message: Message) -> Result<(), Error> {
    match message {
        Message::GetTxPoolInfo(Request { responder, .. }) => {
            reply(responder, pool.pool_info().await, "get_tx_pool_info")
        }
        Message::GetLiveCell(Request {
            responder,
            arguments: (point, data),
        }) => {
            respond(responder, pool.live_cell(&point, data), "get_live_cell");
            Ok(())
        }
        Message::BlockTemplate(Request { responder, .. }) => {
            reply_result(responder, pool.block_template().await, "block_template")
        }
        Message::SubmitLocalTx(Request {
            responder,
            arguments,
        })
        | Message::SubmitLocalTestTx(Request {
            responder,
            arguments,
        }) => reply(
            responder,
            pool.submit_local(arguments, false)
                .await
                .map(|result| result.map(|_| ())),
            "submit_local_tx",
        ),
        Message::TestAcceptTx(Request {
            responder,
            arguments,
        }) => reply(
            responder,
            pool.submit_local(arguments, true).await,
            "test_accept_tx",
        ),
        Message::RemoveLocalTx(Request {
            responder,
            arguments,
        }) => {
            let result = pool
                .remove_local(&arguments)
                .await
                .map(|result| result.map_err(AnyError::from));
            match result {
                Ok(result) => {
                    respond(responder, result, "remove_local_tx");
                    Ok(())
                }
                Err(error) => reply_result(responder, Err(error), "remove_local_tx"),
            }
        }
        Message::SubmitRemoteTx(Request {
            responder,
            arguments:
                RemoteTxSubmission {
                    transaction,
                    declared_cycles,
                    peer,
                },
        }) => reply(
            responder,
            pool.submit_remote(transaction, declared_cycles, peer).await,
            "submit_remote_tx",
        ),
        Message::SubmitRemoteTxBatch(Request {
            responder,
            arguments,
        }) => {
            let (peer, submissions) = arguments.into_parts();
            let offered = submissions.len();
            let (completed, error) = pool.submit_remote_batch(peer, submissions).await;
            let outcome = match &error {
                None => RemoteTxBatchOutcome::complete(offered),
                Some(error) => {
                    RemoteTxBatchOutcome::failed(offered, completed, error.clone().into())
                }
            };
            respond(responder, outcome, "submit_remote_txs");
            error.map_or(Ok(()), settle)
        }
        Message::NotifyTxs(Notify { arguments }) => pool
            .submit_proposal_batch(arguments.into_transactions())
            .await
            .or_else(settle),
        Message::FreshProposalsFilter(Request {
            responder,
            arguments,
        }) => reply(
            responder,
            pool.fresh_proposals(arguments.into_vec()),
            "fresh_proposals_filter",
        ),
        Message::FetchTxs(Request {
            responder,
            arguments,
        }) => reply(
            responder,
            pool.compact_transactions(&arguments.into_vec()),
            "fetch_txs",
        ),
        Message::FetchTxsWithCycles(Request {
            responder,
            arguments,
        }) => reply(
            responder,
            pool.accepted_with_cycles(&arguments.into_vec()),
            "fetch_txs_with_cycles",
        ),
        Message::GetTxStatus(Request {
            responder,
            arguments,
        }) => reply_external(
            responder,
            pool.transaction_status(&arguments),
            "get_tx_status",
        ),
        Message::GetTransactionWithStatus(Request {
            responder,
            arguments,
        }) => reply_external(
            responder,
            pool.transaction(&arguments).await,
            "get_transaction_with_status",
        ),
        Message::NewUncle(Notify { arguments }) => {
            pool.uncle(arguments);
            Ok(())
        }
        Message::GetPoolTxDetails(Request {
            responder,
            arguments,
        }) => reply(
            responder,
            pool.detail(&arguments).await,
            "get_pool_tx_details",
        ),
        Message::GetAllEntryInfo(Request { responder, .. }) => {
            reply(responder, pool.entry_info().await, "get_all_entry_info")
        }
        Message::GetAllIds(Request { responder, .. }) => {
            reply(responder, pool.pool_ids().await, "get_all_ids")
        }
        Message::SavePool(Request { responder, .. }) => {
            if let Err(error) = pool.save().await {
                if let Some(Error::Fault(reason)) = error.downcast_ref::<Error>() {
                    drop(responder);
                    return Err(Error::Fault(reason));
                }
                ckb_logger::error!("explicit tx-pool save failed: {error}");
            }
            respond(responder, (), "save_pool");
            Ok(())
        }
        Message::UpdateIBDState(Request {
            responder,
            arguments,
        }) => {
            pool.ibd(arguments);
            respond(responder, (), "update_ibd_state");
            Ok(())
        }
        Message::EstimateFeeRate(Request {
            responder,
            arguments: (mode, fallback),
        }) => reply_external(
            responder,
            pool.estimate_fee(mode, fallback).await,
            "estimate_fee_rate",
        ),
        Message::GetTotalRecentRejectNum(Request { responder, .. }) => {
            respond(
                responder,
                pool.recent_count(),
                "get_total_recent_reject_num",
            );
            Ok(())
        }
        #[cfg(feature = "internal")]
        Message::PlugEntry(Request {
            responder,
            arguments: (entries, target),
        }) => {
            respond(responder, pool.plug(entries, target).await, "plug_entry");
            Ok(())
        }
        #[cfg(feature = "internal")]
        Message::PackageTxs(Request {
            responder,
            arguments,
        }) => reply(
            responder,
            pool.package_transactions(arguments).await,
            "package_txs",
        ),
    }
}
fn settle(error: Error) -> Result<(), Error> {
    if matches!(error, Error::Fault(_)) {
        Err(error)
    } else {
        ckb_logger::debug!("tx-pool request ended: {error}");
        Ok(())
    }
}
fn reply<R: std::fmt::Debug>(
    responder: impl OneshotSender<R>,
    result: Result<R, Error>,
    name: &'static str,
) -> Result<(), Error> {
    match result {
        Ok(value) => {
            respond(responder, value, name);
            Ok(())
        }
        Err(error) => {
            drop(responder);
            settle(error)
        }
    }
}
fn reply_result<R: std::fmt::Debug>(
    responder: impl OneshotSender<Result<R, AnyError>>,
    result: Result<R, Error>,
    name: &'static str,
) -> Result<(), Error> {
    match result {
        Ok(value) => {
            respond(responder, Ok(value), name);
            Ok(())
        }
        Err(error) => {
            respond(responder, Err(error.clone().into()), name);
            settle(error)
        }
    }
}
fn reply_external<R: std::fmt::Debug>(
    responder: impl OneshotSender<Result<R, AnyError>>,
    result: Result<R, AnyError>,
    name: &'static str,
) -> Result<(), Error> {
    let fault = result
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<Error>())
        .filter(|error| matches!(error, Error::Fault(_)))
        .cloned();
    respond(responder, result, name);
    fault.map_or(Ok(()), Err)
}

#[cfg(test)]
#[path = "tests/dispatch.rs"]
mod tests;
