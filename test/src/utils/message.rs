use ckb_network::bytes::Bytes;
use ckb_types::{
    core::{BlockView, HeaderView, TransactionView},
    packed::{
        BlockTransactions, Byte32, CompactBlock, GetBlocks, RelayMessage, RelayTransaction,
        RelayTransactionHashes, RelayTransactions, SendBlock, SendHeaders, SyncMessage,
    },
    prelude::*,
};

// Build compact block based on core block, and specific prefilled indices
pub fn build_compact_block_with_prefilled(block: &BlockView, prefilled: Vec<usize>) -> Bytes {
    let prefilled = prefilled.into_iter().collect();
    let compact_block = CompactBlock::build_from_block(block, &prefilled);

    RelayMessage::new_builder()
        .set(compact_block)
        .build()
        .as_bytes()
}

// Build compact block based on core block
pub fn build_compact_block(block: &BlockView) -> Bytes {
    build_compact_block_with_prefilled(block, Vec::new())
}

pub fn build_block_transactions(block: &BlockView) -> Bytes {
    // compact block has always prefilled cellbase
    let block_txs = BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(
            block
                .transactions()
                .into_iter()
                .map(|view| view.data())
                .skip(1)
                .pack(),
        )
        .build();

    RelayMessage::new_builder()
        .set(block_txs)
        .build()
        .as_bytes()
}

pub fn build_header(header: &HeaderView) -> Bytes {
    build_headers(&[header.clone()])
}

pub fn build_headers(headers: &[HeaderView]) -> Bytes {
    let send_headers = SendHeaders::new_builder()
        .headers(
            headers
                .iter()
                .map(|view| view.data())
                .collect::<Vec<_>>()
                .pack(),
        )
        .build();

    SyncMessage::new_builder()
        .set(send_headers)
        .build()
        .as_bytes()
}

pub fn build_block(block: &BlockView) -> Bytes {
    SyncMessage::new_builder()
        .set(SendBlock::new_builder().block(block.data()).build())
        .build()
        .as_bytes()
}

pub fn build_get_blocks(hashes: &[Byte32]) -> Bytes {
    let get_blocks = GetBlocks::new_builder()
        .block_hashes(hashes.iter().map(ToOwned::to_owned).pack())
        .build();

    SyncMessage::new_builder()
        .set(get_blocks)
        .build()
        .as_bytes()
}

pub fn build_relay_txs(transactions: &[(TransactionView, u64)]) -> Bytes {
    let transactions = transactions.iter().map(|(tx, cycles)| {
        RelayTransaction::new_builder()
            .cycles(cycles.pack())
            .transaction(tx.data())
            .build()
    });
    let txs = RelayTransactions::new_builder()
        .transactions(transactions.pack())
        .build();

    RelayMessage::new_builder().set(txs).build().as_bytes()
}

pub fn build_relay_tx_hashes(hashes: &[Byte32]) -> Bytes {
    let content = RelayTransactionHashes::new_builder()
        .tx_hashes(hashes.iter().map(ToOwned::to_owned).pack())
        .build();

    RelayMessage::new_builder().set(content).build().as_bytes()
}
