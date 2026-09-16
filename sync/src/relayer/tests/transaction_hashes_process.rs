use crate::relayer::tests::helper::build_chain;
use crate::relayer::transaction_hashes_process::TransactionHashesProcess;
use crate::{Status, StatusCode};
use ckb_constant::sync::{MAX_UNKNOWN_TX_HASHES_SIZE, MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER};
use ckb_network::PeerIndex;
use ckb_types::{packed, prelude::*};
use std::sync::Barrier;

fn tx_hash(index: usize) -> packed::Byte32 {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
    bytes.into()
}

#[test]
fn unknown_transaction_peer_limit_rejects_new_hashes() {
    let (_chain, relayer, _) = build_chain(1);
    let state = relayer.shared.state();
    let peer = PeerIndex::from(1);
    state.peers().relay_connected(peer);
    let limit = MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER;
    assert!(
        state
            .add_ask_for_txs(peer, (0..limit - 1).map(tx_hash).collect())
            .is_ok()
    );

    let status = state.add_ask_for_txs(peer, vec![tx_hash(limit - 1), tx_hash(limit)]);
    assert_eq!(state.unknown_tx_hashes().len(), limit);
    assert!(
        state
            .unknown_tx_hashes()
            .get_priority(&tx_hash(limit))
            .is_none()
    );
    assert_eq!(status.code(), StatusCode::TooManyUnknownTransactions);
    assert!(
        state
            .add_ask_for_txs(peer, vec![tx_hash(0), tx_hash(0)])
            .is_ok(),
        "duplicates do not consume capacity, even at the limit"
    );

    state.mark_as_known_tx(tx_hash(0));
    assert!(state.add_ask_for_txs(peer, vec![tx_hash(limit)]).is_ok());
    assert_eq!(state.unknown_tx_hashes().len(), limit);
}

#[test]
fn unknown_transaction_peer_limit_counts_shared_hashes() {
    let (_chain, relayer, _) = build_chain(1);
    let state = relayer.shared.state();
    let first = PeerIndex::from(1);
    let second = PeerIndex::from(2);
    state.peers().relay_connected(first);
    state.peers().relay_connected(second);
    let limit = MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER;
    assert!(
        state
            .add_ask_for_txs(first, (0..limit - 1).map(tx_hash).collect())
            .is_ok()
    );
    assert!(
        state
            .add_ask_for_txs(second, vec![tx_hash(limit - 1), tx_hash(limit)])
            .is_ok()
    );

    let status = state.add_ask_for_txs(first, vec![tx_hash(limit - 1), tx_hash(limit)]);
    let mut priority = state
        .unknown_tx_hashes()
        .get_priority(&tx_hash(limit))
        .unwrap()
        .clone();
    assert_eq!(priority.next_request_peer(), Some(second));
    assert_eq!(priority.next_request_peer(), None);
    assert_eq!(status.code(), StatusCode::TooManyUnknownTransactions);
}

#[test]
fn unknown_transaction_global_limit_preserves_existing_requests() {
    let (_chain, relayer, _) = build_chain(1);
    let state = relayer.shared.state();
    let first = PeerIndex::from(1);
    let second = PeerIndex::from(2);
    let replacement = PeerIndex::from(3);
    for peer in [first, second, replacement] {
        state.peers().relay_connected(peer);
    }
    let limit = MAX_UNKNOWN_TX_HASHES_SIZE;
    let split = MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER;
    assert!(
        state
            .add_ask_for_txs(first, (0..split).map(tx_hash).collect())
            .is_ok()
    );
    assert!(
        state
            .add_ask_for_txs(second, (split..limit).map(tx_hash).collect())
            .is_ok()
    );

    let status = state.add_ask_for_txs(replacement, vec![tx_hash(limit), tx_hash(0)]);
    assert_eq!(state.unknown_tx_hashes().len(), limit);
    assert!(
        state
            .unknown_tx_hashes()
            .get_priority(&tx_hash(limit))
            .is_none()
    );
    assert_eq!(status, Status::ignored());
    let mut priority = state
        .unknown_tx_hashes()
        .get_priority(&tx_hash(0))
        .unwrap()
        .clone();
    assert_eq!(priority.next_request_peer(), Some(first));
    assert_eq!(priority.next_request_peer(), Some(replacement));
    assert_eq!(priority.next_request_peer(), None);

    state.mark_as_known_tx(tx_hash(1));
    let message = packed::RelayTransactionHashes::new_builder()
        .tx_hashes(vec![tx_hash(limit)])
        .build();
    assert!(
        TransactionHashesProcess::new(message.as_reader(), &relayer, replacement)
            .execute()
            .is_ok(),
        "a refused announcement can be retried once capacity is available"
    );
    assert_eq!(state.unknown_tx_hashes().len(), limit);
}

#[test]
fn unknown_transaction_concurrent_admission_shares_peer_capacity() {
    let (_chain, relayer, _) = build_chain(1);
    let state = relayer.shared.state();
    let peer = PeerIndex::from(1);
    state.peers().relay_connected(peer);
    let limit = MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER;
    assert!(
        state
            .add_ask_for_txs(peer, (0..limit - 1).map(tx_hash).collect())
            .is_ok()
    );

    let ready = Barrier::new(2);
    let statuses = std::thread::scope(|scope| {
        [limit - 1, limit]
            .map(|index| {
                let ready = &ready;
                scope.spawn(move || {
                    ready.wait();
                    state.add_ask_for_txs(peer, vec![tx_hash(index)]).code()
                })
            })
            .map(|task| task.join().unwrap())
    });
    assert_eq!(state.unknown_tx_hashes().len(), limit);
    assert!(statuses.contains(&StatusCode::OK));
    assert!(statuses.contains(&StatusCode::TooManyUnknownTransactions));
}
