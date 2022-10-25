use std::collections::HashSet;

use ckb_network::{CKBProtocolHandler, PeerIndex, SupportProtocols};
use ckb_types::{h256, packed, prelude::*};

use crate::tests::{
    prelude::*,
    utils::{MockChain, MockNetworkContext},
};

#[tokio::test(flavor = "multi_thread")]
async fn get_transactions_proof_with_missing_txs() {
    let chain = MockChain::new();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    chain.mine_to(20);

    let shared = chain.shared();

    let tx1 = chain.get_cellbase_as_input(12);
    let block_contains_tx1 = {
        chain.mine_block(|block| {
            let ids = vec![tx1.proposal_short_id()];
            block.as_advanced_builder().proposals(ids).build()
        });
        chain.mine_blocks(1);
        let num =
            chain.mine_block(|block| block.as_advanced_builder().transaction(tx1.clone()).build());
        chain.mine_blocks(1);
        shared.snapshot().get_block_by_number(num).unwrap()
    };

    let base_header = shared.snapshot().tip_header().to_owned();
    let end_block_number = 40;

    let tx2 = chain.get_cellbase_as_input(13);

    // Spend tx2
    chain.mine_block(|block| {
        let ids = vec![tx2.proposal_short_id()];
        block.as_advanced_builder().proposals(ids).build()
    });
    chain.mine_blocks(1);
    chain.mine_block(|block| block.as_advanced_builder().transaction(tx2.clone()).build());
    chain.mine_to(end_block_number);

    // Rollback
    let mut detached_proposal_ids = HashSet::default();
    detached_proposal_ids.insert(tx2.proposal_short_id());
    chain.rollback_to(base_header.number(), detached_proposal_ids);

    let block_start_number = chain.mine_block(|block| {
        let cellbase = block.transactions().get(0).unwrap();
        block
            .as_advanced_builder()
            .set_transactions(vec![cellbase.into_view()])
            .build()
    });
    assert_eq!(block_start_number, base_header.number() + 1);
    for _ in (base_header.number() + 2)..=end_block_number {
        let _ = chain.mine_block(|block| {
            let cellbase = block.transactions().get(0).unwrap();
            block
                .as_advanced_builder()
                .set_transactions(vec![cellbase.into_view()])
                .build()
        });
    }
    chain.mine_to(end_block_number);

    let tx3_hash = h256!("0x1");

    let expected_missing_tx_hashes = vec![tx2.hash(), tx3_hash.pack()];

    let snapshot = chain.shared().snapshot();

    for num in (base_header.number() + 1)..=end_block_number {
        let new_block = snapshot.get_block_by_number(num).unwrap();
        assert_eq!(new_block.transactions().len(), 1);
    }

    let mut protocol = chain.create_light_client_protocol();

    let data = {
        let content = packed::GetTransactionsProof::new_builder()
            .last_hash(snapshot.tip_header().hash())
            .tx_hashes(vec![tx1.hash(), tx2.hash(), tx3_hash.pack()].pack())
            .build();
        packed::LightClientMessage::new_builder()
            .set(content)
            .build()
    }
    .as_bytes();

    assert!(nc.sent_messages().borrow().is_empty());

    let peer_index = PeerIndex::new(1);
    protocol.received(nc.context(), peer_index, data).await;

    assert!(nc.not_banned(peer_index));

    assert_eq!(nc.sent_messages().borrow().len(), 1);

    let data = &nc.sent_messages().borrow()[0].2;
    let message = packed::LightClientMessageReader::new_unchecked(data);
    let content = if let packed::LightClientMessageUnionReader::SendTransactionsProof(content) =
        message.to_enum()
    {
        content
    } else {
        panic!("unexpected message");
    }
    .to_entity();
    assert_eq!(content.filtered_blocks().len(), 1);
    assert_eq!(content.missing_tx_hashes().len(), 2);
    assert_eq!(
        content
            .filtered_blocks()
            .get(0)
            .unwrap()
            .header()
            .calc_header_hash(),
        block_contains_tx1.hash()
    );
    let missing_tx_hashes = content
        .missing_tx_hashes()
        .into_iter()
        .collect::<HashSet<_>>();
    for hash in &expected_missing_tx_hashes {
        assert!(missing_tx_hashes.contains(hash));
    }
}
