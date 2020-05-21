use ckb_logger::{debug, metric};
use ckb_network::{CKBProtocolContext, Error, PeerIndex};
use ckb_types::packed::{BlockTransactions, GetHeaders, UncleBlock};
use ckb_types::{
    core,
    packed::{
        BlockProposal, Byte32, GetBlockProposal, GetBlockTransactions, GetBlocks,
        GetRelayTransactions, Header, InIBD, ProposalShortId, RelayMessage, RelayTransaction,
        RelayTransactions, SendBlock, SendHeaders, SyncMessage, Transaction,
    },
    prelude::*,
};
use fail::fail_point;

pub fn send_getheaders(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    locator_hashes: Vec<Byte32>,
    hash_stop: Byte32,
) -> Result<(), Error> {
    let content = GetHeaders::new_builder()
        .block_locator_hashes(locator_hashes.pack())
        .hash_stop(hash_stop)
        .build();
    let message = SyncMessage::new_builder().set(content).build();

    debug!("send_getheaders to {}", peer);
    log_sent_sync_metric("getheaders");

    fail_point!("send_getheaders", |_| {
        debug!("[failpoint] send_getheaders to {}", peer);
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_sendheaders(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    headers: Vec<Header>,
) -> Result<(), Error> {
    let length = headers.len();
    let content = SendHeaders::new_builder().headers(headers.pack()).build();
    let message = SyncMessage::new_builder().set(content).build();

    debug!("send_sendheaders(len={}) to {}", length, peer);
    log_sent_sync_metric("sendheaders");

    fail_point!("send_sendheaders", |_| {
        debug!("[failpoint] send_sendheaders(len={}) to {}", length, peer);
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_getblocks(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    hashes: Vec<Byte32>,
) -> Result<(), Error> {
    let length = hashes.len();
    let content = GetBlocks::new_builder().block_hashes(hashes.pack()).build();
    let message = SyncMessage::new_builder().set(content).build();

    debug!("send_getblocks(len={}) to {}", length, peer);
    log_sent_sync_metric("getblocks");

    fail_point!("send_getblocks", |_| {
        debug!("[failpoint] send_getblocks(len={}) to {}", length, peer);
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_sendblock(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    block: core::BlockView,
) -> Result<(), Error> {
    let number = block.number();
    let block_hash = block.hash();
    let content = SendBlock::new_builder().block(block.data()).build();
    let message = SyncMessage::new_builder().set(content).build();

    debug!(
        "send_sendblock(number={}, block_hash={:?}) to {}",
        number, block_hash, peer
    );
    log_sent_sync_metric("sendblock");

    fail_point!("send_sendblock", |_| {
        debug!(
            "[failpoint] send_sendblock(number={}, block_hash={:?}) to {}",
            number, block_hash, peer
        );
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_inibd(nc: &dyn CKBProtocolContext, peer: PeerIndex) -> Result<(), Error> {
    let content = InIBD::new_builder().build();
    let message = SyncMessage::new_builder().set(content).build();

    debug!("send_inibd to {}", peer);
    log_sent_sync_metric("inibd");

    fail_point!("send_inibd", |_| {
        debug!("[failpoint] send_inibd to {}", peer);
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_getblockproposal(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    block_hash: Byte32,
    proposals: Vec<ProposalShortId>,
) -> Result<(), Error> {
    let length = proposals.len();
    let content = GetBlockProposal::new_builder()
        .block_hash(block_hash.clone())
        .proposals(proposals.pack())
        .build();
    let message = RelayMessage::new_builder().set(content).build();

    debug!(
        "send_getblockproposal(block_hash={:?}, len={}) to {}",
        block_hash, length, peer
    );
    log_sent_relay_metric("getblockproposal");

    fail_point!("send_getblockproposal", |_| {
        debug!(
            "[failpoint] send_getblockproposal(block_hash={:?}, len={}) to {}",
            block_hash, length, peer
        );
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_blockproposal(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    transactions: Vec<Transaction>,
) -> Result<(), Error> {
    let length = transactions.len();
    let content = BlockProposal::new_builder()
        .transactions(transactions.pack())
        .build();
    let message = RelayMessage::new_builder().set(content).build();

    debug!("send_blockproposal(len={}) to {}", length, peer);
    log_sent_relay_metric("blockproposal");

    fail_point!("send_blockproposal", |_| {
        debug!("[failpoint] send_blockproposal(len={}) to {}", length, peer);
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_relaytransactions(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    transactions: Vec<RelayTransaction>,
) -> Result<(), Error> {
    let length = transactions.len();
    let content = RelayTransactions::new_builder()
        .transactions(transactions.pack())
        .build();
    let message = RelayMessage::new_builder().set(content).build();

    debug!("send_relaytransactions(len={}) to {}", length, peer);
    log_sent_relay_metric("relaytransactions");

    fail_point!("send_relaytransactions", |_| {
        debug!(
            "[failpoint] send_relaytransactions(len={}) to {}",
            length, peer
        );
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_getblocktransactions(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    block_hash: Byte32,
    indexes: Vec<u32>,
    uncle_indexes: Vec<u32>,
) -> Result<(), Error> {
    let indexes_length = indexes.len();
    let uncle_indexes_length = uncle_indexes.len();
    let content = GetBlockTransactions::new_builder()
        .block_hash(block_hash.clone())
        .indexes(indexes.pack())
        .uncle_indexes(uncle_indexes.pack())
        .build();
    let message = RelayMessage::new_builder().set(content).build();

    debug!(
        "send_getblocktransactions(block_hash: {:?}, indexes_len={}, uncle_indexes_len={}) to {}",
        block_hash, indexes_length, uncle_indexes_length, peer
    );
    log_sent_relay_metric("getBlocktransactions");

    fail_point!("send_getblocktransactions", |_| {
        debug!("[failpoint] send_getblocktransactions(block_hash: {:?}, indexes_len={}, uncle_indexes_len={}) to {}", block_hash, indexes_length, uncle_indexes_length, peer);
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_blocktransactions(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    block_hash: Byte32,
    transactions: Vec<Transaction>,
    uncles: Vec<UncleBlock>,
) -> Result<(), Error> {
    let indexes_length = transactions.len();
    let uncle_indexes_length = uncles.len();
    let content = BlockTransactions::new_builder()
        .block_hash(block_hash.clone())
        .transactions(transactions.pack())
        .uncles(uncles.pack())
        .build();
    let message = RelayMessage::new_builder().set(content).build();

    debug!(
        "send_blocktransactions(block_hash: {:?}, indexes_len={}, uncle_indexes_len={}) to {}",
        block_hash, indexes_length, uncle_indexes_length, peer
    );
    log_sent_relay_metric("getBlocktransactions");

    fail_point!("send_blocktransactions", |_| {
        debug!(
            "[failpoint] send_blocktransactions(block_hash: {:?}, indexes_len={}, uncle_indexes_len={}) to {}",
            block_hash, indexes_length, uncle_indexes_length, peer,
        );
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

pub fn send_getrelaytransactions(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    tx_hashes: Vec<Byte32>,
) -> Result<(), Error> {
    let length = tx_hashes.len();
    let content = GetRelayTransactions::new_builder()
        .tx_hashes(tx_hashes.pack())
        .build();
    let message = RelayMessage::new_builder().set(content).build();

    debug!("send_getrelaytransactions(len={}) to {}", length, peer);
    log_sent_relay_metric("getrelaytransactions");

    fail_point!("send_getrelaytransactions", |_| {
        debug!(
            "[failpoint] send_getrelaytransactions(len={}) to {}",
            length, peer
        );
        Ok(())
    });

    nc.send_message_to(peer, message.as_bytes())
}

fn log_sent_sync_metric(item_name: &str) {
    metric!({
        "topic": "sent",
        "fields": { item_name: 1 }
    });
}

fn log_sent_relay_metric(item_name: &str) {
    metric!({
        "topic": "sent",
        "tags": { "target": crate::LOG_TARGET_RELAY },
        "fields": { item_name: 1 }
    });
}
