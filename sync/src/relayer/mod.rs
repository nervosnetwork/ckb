mod block_proposal_process;
mod block_transactions_process;
mod block_transactions_verifier;
mod block_uncles_verifier;
mod compact_block_process;
mod compact_block_verifier;
mod get_block_proposal_process;
mod get_block_transactions_process;
mod get_transactions_process;
#[cfg(test)]
mod tests;
mod transaction_hashes_process;
mod transactions_process;

use self::block_proposal_process::BlockProposalProcess;
use self::block_transactions_process::BlockTransactionsProcess;
use self::compact_block_process::CompactBlockProcess;
use self::get_block_proposal_process::GetBlockProposalProcess;
use self::get_block_transactions_process::GetBlockTransactionsProcess;
use self::get_transactions_process::GetTransactionsProcess;
use self::transaction_hashes_process::TransactionHashesProcess;
use self::transactions_process::TransactionsProcess;
use crate::block_status::BlockStatus;
use crate::types::{ActiveChain, BlockNumberAndHash, SyncShared};
use crate::utils::send_message_to;
use crate::{Status, StatusCode};
use ckb_chain::chain::ChainController;
use ckb_constant::sync::BAD_MESSAGE_BAN_TIME;
use ckb_logger::{debug_target, error_target, info_target, trace_target, warn_target};
use ckb_metrics::metrics;
use ckb_network::{
    async_trait, bytes::Bytes, tokio, CKBProtocolContext, CKBProtocolHandler, PeerIndex,
    SupportProtocols, TargetSession,
};
use ckb_store::ChainStore;
use ckb_tx_pool::service::TxVerificationResult;
use ckb_types::{
    core::{self, BlockView},
    packed::{self, Byte32, ProposalShortId},
    prelude::*,
};
use ckb_util::Mutex;
use faketime::unix_time_as_millis;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const TX_PROPOSAL_TOKEN: u64 = 0;
pub const ASK_FOR_TXS_TOKEN: u64 = 1;
pub const TX_HASHES_TOKEN: u64 = 2;
pub const SEARCH_ORPHAN_POOL_TOKEN: u64 = 3;

pub const MAX_RELAY_PEERS: usize = 128;
pub const MAX_RELAY_TXS_NUM_PER_BATCH: usize = 32767;
pub const MAX_RELAY_TXS_BYTES_PER_BATCH: usize = 1024 * 1024;

type RateLimiter<T> = governor::RateLimiter<
    T,
    governor::state::keyed::DefaultKeyedStateStore<T>,
    governor::clock::DefaultClock,
>;

#[derive(Debug, Eq, PartialEq)]
pub enum ReconstructionResult {
    Block(BlockView),
    Missing(Vec<usize>, Vec<usize>),
    Collided,
    Error(Status),
}

/// Relayer protocol handle
#[derive(Clone)]
pub struct Relayer {
    chain: ChainController,
    pub(crate) shared: Arc<SyncShared>,
    rate_limiter: Arc<Mutex<RateLimiter<(PeerIndex, u32)>>>,
}

impl Relayer {
    /// Init relay protocol handle
    ///
    /// This is a runtime relay protocol shared state, and any relay messages will be processed and forwarded by it
    pub fn new(chain: ChainController, shared: Arc<SyncShared>) -> Self {
        // setup a rate limiter keyed by peer and message type that lets through 30 requests per second
        // current max rps is 10 (ASK_FOR_TXS_TOKEN / TX_PROPOSAL_TOKEN), 30 is a flexible hard cap with buffer
        let quota = governor::Quota::per_second(std::num::NonZeroU32::new(30).unwrap());
        let rate_limiter = Arc::new(Mutex::new(RateLimiter::keyed(quota)));
        Relayer {
            chain,
            shared,
            rate_limiter,
        }
    }

    /// Get shared state
    pub fn shared(&self) -> &Arc<SyncShared> {
        &self.shared
    }

    fn try_process(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: packed::RelayMessageUnionReader<'_>,
    ) -> Status {
        // CompactBlock will be verified by POW, it's OK to skip rate limit checking.
        let should_check_rate =
            !matches!(message, packed::RelayMessageUnionReader::CompactBlock(_));

        if should_check_rate
            && self
                .rate_limiter
                .lock()
                .check_key(&(peer, message.item_id()))
                .is_err()
        {
            return StatusCode::TooManyRequests.with_context(message.item_name());
        }

        match message {
            packed::RelayMessageUnionReader::CompactBlock(reader) => {
                CompactBlockProcess::new(reader, self, nc, peer).execute()
            }
            packed::RelayMessageUnionReader::RelayTransactions(reader) => {
                if reader.check_data() {
                    TransactionsProcess::new(reader, self, nc, peer).execute()
                } else {
                    StatusCode::ProtocolMessageIsMalformed
                        .with_context("RelayTransactions is invalid")
                }
            }
            packed::RelayMessageUnionReader::RelayTransactionHashes(reader) => {
                TransactionHashesProcess::new(reader, self, peer).execute()
            }
            packed::RelayMessageUnionReader::GetRelayTransactions(reader) => {
                GetTransactionsProcess::new(reader, self, nc, peer).execute()
            }
            packed::RelayMessageUnionReader::GetBlockTransactions(reader) => {
                GetBlockTransactionsProcess::new(reader, self, nc, peer).execute()
            }
            packed::RelayMessageUnionReader::BlockTransactions(reader) => {
                if reader.check_data() {
                    BlockTransactionsProcess::new(reader, self, nc, peer).execute()
                } else {
                    StatusCode::ProtocolMessageIsMalformed
                        .with_context("BlockTransactions is invalid")
                }
            }
            packed::RelayMessageUnionReader::GetBlockProposal(reader) => {
                GetBlockProposalProcess::new(reader, self, nc, peer).execute()
            }
            packed::RelayMessageUnionReader::BlockProposal(reader) => {
                BlockProposalProcess::new(reader, self).execute()
            }
        }
    }

    fn process(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: packed::RelayMessageUnionReader<'_>,
    ) {
        let item_name = message.item_name();
        let item_bytes = message.as_slice().len() as u64;
        let status = self.try_process(Arc::clone(&nc), peer, message);

        metrics!(
            counter,
            "ckb.messages_bytes",
            item_bytes,
            "direction" => "in",
            "protocol_id" => SupportProtocols::RelayV2.protocol_id().value().to_string(),
            "item_id" => message.item_id().to_string(),
            "status" => (status.code() as u16).to_string(),
        );

        if let Some(ban_time) = status.should_ban() {
            error_target!(
                crate::LOG_TARGET_RELAY,
                "receive {} from {}, ban {:?} for {}",
                item_name,
                peer,
                ban_time,
                status
            );
            nc.ban_peer(peer, ban_time, status.to_string());
        } else if status.should_warn() {
            warn_target!(
                crate::LOG_TARGET_RELAY,
                "receive {} from {}, {}",
                item_name,
                peer,
                status
            );
        } else if !status.is_ok() {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "receive {} from {}, {}",
                item_name,
                peer,
                status
            );
        }
    }

    /// Request the transaction corresponding to the proposal id from the specified node
    pub fn request_proposal_txs(
        &self,
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
        block_hash_and_number: BlockNumberAndHash,
        mut proposals: Vec<packed::ProposalShortId>,
    ) {
        proposals.dedup();
        let tx_pool = self.shared.shared().tx_pool_controller();
        let fresh_proposals = match tx_pool.fresh_proposals_filter(proposals) {
            Err(err) => {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "tx_pool fresh_proposals_filter error: {:?}",
                    err,
                );
                return;
            }
            Ok(fresh_proposals) => fresh_proposals,
        };

        let to_ask_proposals: Vec<ProposalShortId> = self
            .shared()
            .state()
            .insert_inflight_proposals(fresh_proposals.clone(), block_hash_and_number.number)
            .into_iter()
            .zip(fresh_proposals)
            .filter_map(|(firstly_in, id)| if firstly_in { Some(id) } else { None })
            .collect();
        if !to_ask_proposals.is_empty() {
            let content = packed::GetBlockProposal::new_builder()
                .block_hash(block_hash_and_number.hash)
                .proposals(to_ask_proposals.clone().pack())
                .build();
            let message = packed::RelayMessage::new_builder().set(content).build();
            if !send_message_to(nc, peer, &message).is_ok() {
                self.shared()
                    .state()
                    .remove_inflight_proposals(&to_ask_proposals);
            }
        }
    }

    /// Accept a new block from network
    #[allow(clippy::needless_collect)]
    pub fn accept_block(
        &self,
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
        block: core::BlockView,
    ) {
        if self
            .shared()
            .active_chain()
            .contains_block_status(&block.hash(), BlockStatus::BLOCK_STORED)
        {
            return;
        }

        let boxed = Arc::new(block);
        if self
            .shared()
            .insert_new_block(&self.chain, Arc::clone(&boxed))
            .unwrap_or(false)
        {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "[block_relay] relayer accept_block {} {}",
                boxed.header().hash(),
                unix_time_as_millis()
            );
            let block_hash = boxed.hash();
            self.shared().state().remove_header_view(&block_hash);
            let cb = packed::CompactBlock::build_from_block(&boxed, &HashSet::new());
            let message = packed::RelayMessage::new_builder().set(cb).build();

            let selected_peers: Vec<PeerIndex> = nc
                .connected_peers()
                .into_iter()
                .filter(|target_peer| peer != *target_peer)
                .take(MAX_RELAY_PEERS)
                .collect();
            if let Err(err) = nc.quick_filter_broadcast(
                TargetSession::Multi(Box::new(selected_peers.into_iter())),
                message.as_bytes(),
            ) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send block when accept block error: {:?}",
                    err,
                );
            }

            if let Some(p2p_control) = nc.p2p_control() {
                let tip_header = packed::VerifiableHeader::new_builder()
                    .header(boxed.header().data())
                    .uncles_hash(boxed.calc_uncles_hash())
                    .extension(Pack::pack(&boxed.extension()))
                    .build();
                let total_difficulty = self
                    .shared
                    .shared()
                    .snapshot()
                    .get_block_ext(&block_hash)
                    .map(|block_ext| block_ext.total_difficulty)
                    .expect("checked: new block should have block ext");
                let light_client_message = {
                    let content = packed::SendLastState::new_builder()
                        .tip_header(tip_header)
                        .total_difficulty(total_difficulty.pack())
                        .build();
                    packed::LightClientMessage::new_builder()
                        .set(content)
                        .build()
                };
                let light_client_peers: HashSet<PeerIndex> = nc
                    .connected_peers()
                    .into_iter()
                    .filter_map(|index| nc.get_peer(index).map(|peer| (index, peer)))
                    .filter(|(_id, peer)| peer.if_lightclient_subscribed)
                    .map(|(id, _)| id)
                    .collect();
                if let Err(err) = p2p_control.filter_broadcast(
                    TargetSession::Filter(Box::new(move |id| light_client_peers.contains(id))),
                    SupportProtocols::LightClient.protocol_id(),
                    light_client_message.as_bytes(),
                ) {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "relayer send last state to light client when accept block, error: {:?}",
                        err,
                    );
                }
            }
        }
    }

    /// Reorganize the full block according to the compact block/txs/uncles
    // nodes should attempt to reconstruct the full block by taking the prefilledtxn transactions
    // from the original CompactBlock message and placing them in the marked positions,
    // then for each short transaction ID from the original compact_block message, in order,
    // find the corresponding transaction either from the BlockTransactions message or
    // from other sources and place it in the first available position in the block
    // then once the block has been reconstructed, it shall be processed as normal,
    // keeping in mind that short_ids are expected to occasionally collide,
    // and that nodes must not be penalized for such collisions, wherever they appear.
    pub fn reconstruct_block(
        &self,
        active_chain: &ActiveChain,
        compact_block: &packed::CompactBlock,
        received_transactions: Vec<core::TransactionView>,
        uncles_index: &[u32],
        received_uncles: &[core::UncleBlockView],
    ) -> ReconstructionResult {
        let block_txs_len = received_transactions.len();
        let compact_block_hash = compact_block.calc_header_hash();
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "start block reconstruction, block hash: {}, received transactions len: {}",
            compact_block_hash,
            block_txs_len,
        );

        let mut short_ids_set: HashSet<ProposalShortId> =
            compact_block.short_ids().into_iter().collect();

        let mut txs_map: HashMap<ProposalShortId, core::TransactionView> = received_transactions
            .into_iter()
            .filter_map(|tx| {
                let short_id = tx.proposal_short_id();
                if short_ids_set.remove(&short_id) {
                    Some((short_id, tx))
                } else {
                    None
                }
            })
            .collect();

        if !short_ids_set.is_empty() {
            let tx_pool = self.shared.shared().tx_pool_controller();

            let fetch_txs = tx_pool.fetch_txs(short_ids_set);
            if let Err(e) = fetch_txs {
                return ReconstructionResult::Error(StatusCode::TxPool.with_context(e));
            }
            txs_map.extend(fetch_txs.unwrap().into_iter());
        }

        let txs_len = compact_block.txs_len();
        let mut block_transactions: Vec<Option<core::TransactionView>> =
            Vec::with_capacity(txs_len);

        let short_ids_iter = &mut compact_block.short_ids().into_iter();
        // fill transactions gap
        compact_block
            .prefilled_transactions()
            .into_iter()
            .for_each(|pt| {
                let index: usize = pt.index().unpack();
                let gap = index - block_transactions.len();
                if gap > 0 {
                    short_ids_iter
                        .take(gap)
                        .for_each(|short_id| block_transactions.push(txs_map.remove(&short_id)));
                }
                block_transactions.push(Some(pt.transaction().into_view()));
            });

        // append remain transactions
        short_ids_iter.for_each(|short_id| block_transactions.push(txs_map.remove(&short_id)));

        let missing = block_transactions.iter().any(Option::is_none);

        let mut missing_uncles = Vec::with_capacity(compact_block.uncles().len());
        let mut uncles = Vec::with_capacity(compact_block.uncles().len());

        let mut position = 0;
        for (i, uncle_hash) in compact_block.uncles().into_iter().enumerate() {
            if uncles_index.contains(&(i as u32)) {
                uncles.push(
                    received_uncles
                        .get(position)
                        .expect("have checked the indexes")
                        .clone()
                        .data(),
                );
                position += 1;
                continue;
            };
            let status = active_chain.get_block_status(&uncle_hash);
            match status {
                BlockStatus::UNKNOWN | BlockStatus::HEADER_VALID => missing_uncles.push(i),
                BlockStatus::BLOCK_STORED | BlockStatus::BLOCK_VALID => {
                    if let Some(uncle) = active_chain.get_block(&uncle_hash) {
                        uncles.push(uncle.as_uncle().data());
                    } else {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "reconstruct_block could not find {:#?} uncle block: {:#?}",
                            status,
                            uncle_hash,
                        );
                        missing_uncles.push(i);
                    }
                }
                BlockStatus::BLOCK_RECEIVED => {
                    if let Some(uncle) = self.shared.state().get_orphan_block(&uncle_hash) {
                        uncles.push(uncle.as_uncle().data());
                    } else {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "reconstruct_block could not find {:#?} uncle block: {:#?}",
                            status,
                            uncle_hash,
                        );
                        missing_uncles.push(i);
                    }
                }
                BlockStatus::BLOCK_INVALID => {
                    return ReconstructionResult::Error(
                        StatusCode::CompactBlockHasInvalidUncle.with_context(uncle_hash),
                    )
                }
                _ => missing_uncles.push(i),
            }
        }

        if !missing && missing_uncles.is_empty() {
            let txs = block_transactions
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .expect("missing checked, should not fail");
            let block = if let Some(extension) = compact_block.extension() {
                packed::BlockV1::new_builder()
                    .header(compact_block.header())
                    .uncles(uncles.pack())
                    .transactions(txs.into_iter().map(|tx| tx.data()).pack())
                    .proposals(compact_block.proposals())
                    .extension(extension)
                    .build()
                    .as_v0()
            } else {
                packed::Block::new_builder()
                    .header(compact_block.header())
                    .uncles(uncles.pack())
                    .transactions(txs.into_iter().map(|tx| tx.data()).pack())
                    .proposals(compact_block.proposals())
                    .build()
            }
            .into_view();

            debug_target!(
                crate::LOG_TARGET_RELAY,
                "finish block reconstruction, block hash: {}",
                compact_block.calc_header_hash(),
            );

            let compact_block_tx_root = compact_block.header().raw().transactions_root();
            let reconstruct_block_tx_root = block.transactions_root();
            if compact_block_tx_root != reconstruct_block_tx_root {
                if compact_block.short_ids().is_empty()
                    || compact_block.short_ids().len() == block_txs_len
                {
                    return ReconstructionResult::Error(
                        StatusCode::CompactBlockHasUnmatchedTransactionRootWithReconstructedBlock
                            .with_context(format!(
                                "Compact_block_tx_root({}) != reconstruct_block_tx_root({})",
                                compact_block.header().raw().transactions_root(),
                                block.transactions_root(),
                            )),
                    );
                } else {
                    metrics!(counter, "ckb.relay.transaction_short_id_collide", 1);
                    return ReconstructionResult::Collided;
                }
            }

            ReconstructionResult::Block(block)
        } else {
            let missing_indexes: Vec<usize> = block_transactions
                .iter()
                .enumerate()
                .filter_map(|(i, t)| if t.is_none() { Some(i) } else { None })
                .collect();

            debug_target!(
                crate::LOG_TARGET_RELAY,
                "block reconstruction failed, block hash: {}, missing: {}, total: {}",
                compact_block.calc_header_hash(),
                missing_indexes.len(),
                compact_block.short_ids().len(),
            );

            ReconstructionResult::Missing(missing_indexes, missing_uncles)
        }
    }

    fn prune_tx_proposal_request(&self, nc: &dyn CKBProtocolContext) {
        let get_block_proposals = self.shared().state().drain_get_block_proposals();
        let tx_pool = self.shared.shared().tx_pool_controller();

        let fetch_txs = tx_pool.fetch_txs(
            get_block_proposals
                .iter()
                .map(|kv_pair| kv_pair.key().clone())
                .collect(),
        );
        if let Err(err) = fetch_txs {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "relayer prune_tx_proposal_request internal error: {:?}",
                err,
            );
            return;
        }

        let txs = fetch_txs.unwrap();

        let mut peer_txs = HashMap::new();
        for (id, peer_indices) in get_block_proposals.into_iter() {
            if let Some(tx) = txs.get(&id) {
                for peer_index in peer_indices {
                    let tx_set = peer_txs.entry(peer_index).or_insert_with(Vec::new);
                    tx_set.push(tx.clone());
                }
            }
        }

        let send_block_proposals =
            |nc: &dyn CKBProtocolContext, peer_index: PeerIndex, txs: Vec<packed::Transaction>| {
                let content = packed::BlockProposal::new_builder()
                    .transactions(txs.into_iter().pack())
                    .build();
                let message = packed::RelayMessage::new_builder().set(content).build();
                let status = send_message_to(nc, peer_index, &message);
                if !status.is_ok() {
                    ckb_logger::error!(
                        "send RelayBlockProposal to {}, status: {:?}",
                        peer_index,
                        status
                    );
                }
            };

        let mut relay_bytes = 0;
        let mut relay_proposals = Vec::new();
        for (peer_index, txs) in peer_txs {
            for tx in txs {
                let data = tx.data();
                let tx_size = data.total_size();
                if relay_bytes + tx_size > MAX_RELAY_TXS_BYTES_PER_BATCH {
                    send_block_proposals(nc, peer_index, std::mem::take(&mut relay_proposals));
                    relay_bytes = tx_size;
                } else {
                    relay_bytes += tx_size;
                }
                relay_proposals.push(data);
            }
            if !relay_proposals.is_empty() {
                send_block_proposals(nc, peer_index, std::mem::take(&mut relay_proposals));
                relay_bytes = 0;
            }
        }
    }

    /// Ask for relay transaction by hash from all peers
    pub fn ask_for_txs(&self, nc: &dyn CKBProtocolContext) {
        for (peer, mut tx_hashes) in self.shared().state().pop_ask_for_txs() {
            if !tx_hashes.is_empty() {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "Send get transaction ({} hashes) to {}",
                    tx_hashes.len(),
                    peer,
                );
                tx_hashes.truncate(MAX_RELAY_TXS_NUM_PER_BATCH);
                let content = packed::GetRelayTransactions::new_builder()
                    .tx_hashes(tx_hashes.pack())
                    .build();
                let message = packed::RelayMessage::new_builder().set(content).build();
                let status = send_message_to(nc, peer, &message);
                if !status.is_ok() {
                    ckb_logger::error!("break asking for transactions, status: {:?}", status);
                }
            }
        }
    }

    /// Send bulk of tx hashes to selected peers
    pub fn send_bulk_of_tx_hashes(&self, nc: &dyn CKBProtocolContext) {
        const BUFFER_SIZE: usize = 42;

        let connected_peers = nc.connected_peers();
        if connected_peers.is_empty() {
            return;
        }

        let tx_verify_results = self
            .shared
            .state()
            .take_relay_tx_verify_results(MAX_RELAY_TXS_NUM_PER_BATCH);
        let mut selected: HashMap<PeerIndex, Vec<Byte32>> = HashMap::default();
        {
            for tx_verify_result in tx_verify_results {
                match tx_verify_result {
                    TxVerificationResult::Ok {
                        original_peer,
                        tx_hash,
                    } => {
                        for target in &connected_peers {
                            match original_peer {
                                Some(peer) => {
                                    // broadcast tx hash to all connected peers except original peer
                                    if peer != *target {
                                        let hashes = selected
                                            .entry(*target)
                                            .or_insert_with(|| Vec::with_capacity(BUFFER_SIZE));
                                        hashes.push(tx_hash.clone());
                                    }
                                }
                                None => {
                                    // since this tx is submitted through local rpc, it is assumed to be a new tx for all connected peers
                                    let hashes = selected
                                        .entry(*target)
                                        .or_insert_with(|| Vec::with_capacity(BUFFER_SIZE));
                                    hashes.push(tx_hash.clone());
                                    self.shared.state().mark_as_known_tx(tx_hash.clone());
                                }
                            }
                        }
                    }
                    TxVerificationResult::Reject { tx_hash } => {
                        self.shared.state().remove_from_known_txs(&tx_hash);
                    }
                }
            }
        }
        for (peer, hashes) in selected {
            let content = packed::RelayTransactionHashes::new_builder()
                .tx_hashes(hashes.pack())
                .build();
            let message = packed::RelayMessage::new_builder().set(content).build();

            if let Err(err) = nc.filter_broadcast(TargetSession::Single(peer), message.as_bytes()) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send TransactionHashes error: {:?}",
                    err,
                );
            }
        }
    }
}

#[async_trait]
impl CKBProtocolHandler for Relayer {
    async fn init(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>) {
        nc.set_notify(Duration::from_millis(100), TX_PROPOSAL_TOKEN)
            .await
            .expect("set_notify at init is ok");
        nc.set_notify(Duration::from_millis(100), ASK_FOR_TXS_TOKEN)
            .await
            .expect("set_notify at init is ok");
        nc.set_notify(Duration::from_millis(300), TX_HASHES_TOKEN)
            .await
            .expect("set_notify at init is ok");
        // todo: remove when the asynchronous verification is completed
        nc.set_notify(Duration::from_secs(5), SEARCH_ORPHAN_POOL_TOKEN)
            .await
            .expect("set_notify at init is ok");
    }

    async fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: Bytes,
    ) {
        // If self is in the IBD state, don't process any relayer message.
        if self.shared.active_chain().is_initial_block_download() {
            return;
        }

        let msg = match packed::RelayMessageReader::from_compatible_slice(&data) {
            Ok(msg) => {
                let item = msg.to_enum();
                if let packed::RelayMessageUnionReader::CompactBlock(ref reader) = item {
                    if reader.count_extra_fields() > 1 {
                        info_target!(
                            crate::LOG_TARGET_RELAY,
                            "Peer {} sends us a malformed message: \
                             too many fields in CompactBlock",
                            peer_index
                        );
                        nc.ban_peer(
                            peer_index,
                            BAD_MESSAGE_BAN_TIME,
                            String::from(
                                "send us a malformed message: \
                                 too many fields in CompactBlock",
                            ),
                        );
                        return;
                    } else {
                        item
                    }
                } else {
                    match packed::RelayMessageReader::from_slice(&data) {
                        Ok(msg) => msg.to_enum(),
                        _ => {
                            info_target!(
                                crate::LOG_TARGET_RELAY,
                                "Peer {} sends us a malformed message: \
                                 too many fields",
                                peer_index
                            );
                            nc.ban_peer(
                                peer_index,
                                BAD_MESSAGE_BAN_TIME,
                                String::from(
                                    "send us a malformed message \
                                     too many fields",
                                ),
                            );
                            return;
                        }
                    }
                }
            }
            _ => {
                info_target!(
                    crate::LOG_TARGET_RELAY,
                    "Peer {} sends us a malformed message",
                    peer_index
                );
                nc.ban_peer(
                    peer_index,
                    BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };

        debug_target!(
            crate::LOG_TARGET_RELAY,
            "received msg {} from {}",
            msg.item_name(),
            peer_index
        );
        #[cfg(feature = "with_sentry")]
        {
            let sentry_hub = sentry::Hub::current();
            let _scope_guard = sentry_hub.push_scope();
            sentry_hub.configure_scope(|scope| {
                scope.set_tag("p2p.protocol", "relayer");
                scope.set_tag("p2p.message", msg.item_name());
            });
        }

        let start_time = Instant::now();
        tokio::task::block_in_place(|| self.process(nc, peer_index, msg));
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "process message={}, peer={}, cost={:?}",
            msg.item_name(),
            peer_index,
            Instant::now().saturating_duration_since(start_time),
        );
    }

    async fn connected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        version: &str,
    ) {
        self.shared().state().peers().relay_connected(peer_index);
        info_target!(
            crate::LOG_TARGET_RELAY,
            "RelayProtocol({}).connected peer={}",
            version,
            peer_index
        );
    }

    async fn disconnected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
    ) {
        info_target!(
            crate::LOG_TARGET_RELAY,
            "RelayProtocol.disconnected peer={}",
            peer_index
        );
        // Retains all keys in the rate limiter that were used recently enough.
        self.rate_limiter.lock().retain_recent();
    }

    async fn notify(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, token: u64) {
        // If self is in the IBD state, don't trigger any relayer notify.
        if self.shared.active_chain().is_initial_block_download() {
            return;
        }

        let start_time = Instant::now();
        trace_target!(crate::LOG_TARGET_RELAY, "start notify token={}", token);
        match token {
            TX_PROPOSAL_TOKEN => {
                tokio::task::block_in_place(|| self.prune_tx_proposal_request(nc.as_ref()))
            }
            ASK_FOR_TXS_TOKEN => self.ask_for_txs(nc.as_ref()),
            TX_HASHES_TOKEN => self.send_bulk_of_tx_hashes(nc.as_ref()),
            SEARCH_ORPHAN_POOL_TOKEN => {
                if !self.shared.state().orphan_pool().is_empty() {
                    tokio::task::block_in_place(|| {
                        self.shared.try_search_orphan_pool(&self.chain);
                        self.shared.periodic_clean_orphan_pool();
                    })
                }
            }
            _ => unreachable!(),
        }
        trace_target!(
            crate::LOG_TARGET_RELAY,
            "finished notify token={} cost={:?}",
            token,
            Instant::now().saturating_duration_since(start_time)
        );
    }
}
