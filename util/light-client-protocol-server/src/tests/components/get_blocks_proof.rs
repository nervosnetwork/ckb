use std::collections::HashSet;

use ckb_network::{CKBProtocolHandler, PeerIndex, SupportProtocols};
use ckb_types::{h256, packed, prelude::*};

use crate::tests::{
    prelude::*,
    utils::{MockChain, MockNetworkContext},
};

#[tokio::test(flavor = "multi_thread")]
async fn get_blocks_proof_with_missing_blocks() {
    let chain = MockChain::new();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    chain.mine_to(20);

    let shared = chain.shared();
    let base_header = shared.snapshot().tip_header().to_owned();
    let end_block_number = 40;

    chain.mine_to(end_block_number);

    let expected_missing_block_hashes = {
        let mut forked_block_hashes = ((base_header.number() + 1)..=end_block_number)
            .into_iter()
            .map(|num| shared.snapshot().get_header_by_number(num).unwrap().hash())
            .collect::<Vec<_>>();
        let invalid_block_hashes = vec![
            h256!("0x1"),
            h256!("0x2"),
            h256!("0x3"),
            h256!("0x4"),
            h256!("0x5"),
        ]
        .into_iter()
        .map(|h| h.pack());
        forked_block_hashes.extend(invalid_block_hashes);
        forked_block_hashes
    };

    // Rollback
    chain.rollback_to(base_header.number(), HashSet::default());

    // Spend tx
    let tx = chain.get_cellbase_as_input(12);
    chain.mine_block(|block| {
        let ids = vec![tx.proposal_short_id()];
        block.as_advanced_builder().proposals(ids).build()
    });
    chain.mine_blocks(1);
    chain.mine_block(|block| block.as_advanced_builder().transaction(tx.clone()).build());
    chain.mine_to(end_block_number);

    let snapshot = chain.shared().snapshot();

    let mut protocol = chain.create_light_client_protocol();

    let data = {
        let mut request_block_hashes = ((base_header.number() + 1)..end_block_number)
            .into_iter()
            .map(|num| shared.snapshot().get_header_by_number(num).unwrap().hash())
            .collect::<Vec<_>>();
        request_block_hashes.extend_from_slice(&expected_missing_block_hashes[..]);
        let content = packed::GetBlocksProof::new_builder()
            .last_hash(snapshot.tip_header().hash())
            .block_hashes(request_block_hashes.pack())
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
    let content = if let packed::LightClientMessageUnionReader::SendBlocksProof(content) =
        message.to_enum()
    {
        content
    } else {
        panic!("unexpected message");
    }
    .to_entity();
    assert_eq!(
        content.missing_block_hashes().len(),
        expected_missing_block_hashes.len()
    );
    let missing_block_hashes = content
        .missing_block_hashes()
        .into_iter()
        .collect::<HashSet<_>>();
    for hash in &expected_missing_block_hashes {
        assert!(missing_block_hashes.contains(hash));
    }
}
