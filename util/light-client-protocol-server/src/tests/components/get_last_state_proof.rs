use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, leaf_index_to_pos};
use ckb_network::{CKBProtocolHandler, PeerIndex, SupportProtocols};
use ckb_types::{
    packed,
    prelude::*,
    utilities::merkle_mountain_range::{MMRProof, VerifiableHeader},
};

use crate::tests::{
    prelude::*,
    utils::{MockChain, MockNetworkContext},
};

#[tokio::test(flavor = "multi_thread")]
async fn get_last_state_proof_with_the_genesis_block() {
    let chain = MockChain::new();
    let nc = MockNetworkContext::new(SupportProtocols::LightClient);

    chain.mine_to(1);

    let snapshot = chain.shared().snapshot();
    let verifiable_tip_header: VerifiableHeader =
        snapshot.get_verifiable_header_by_number(1).unwrap().into();
    let tip_header = verifiable_tip_header.header();
    let genesis_header = snapshot.get_header_by_number(0).unwrap();

    let mut protocol = chain.create_light_client_protocol();

    let data = {
        let content = packed::GetLastStateProof::new_builder()
            .last_hash(tip_header.hash())
            .start_hash(genesis_header.hash())
            .start_number(0u64.pack())
            .last_n_blocks(10u64.pack())
            .difficulty_boundary(genesis_header.difficulty().pack())
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
    let content = if let packed::LightClientMessageUnionReader::SendLastStateProof(content) =
        message.to_enum()
    {
        content
    } else {
        panic!("unexpected message");
    }
    .to_entity();

    // Verify MMR Proof
    {
        let parent_chain_root = verifiable_tip_header.parent_chain_root();
        let proof: MMRProof = {
            let mmr_size = leaf_index_to_mmr_size(parent_chain_root.end_number().unpack());
            let proof = content.proof().into_iter().collect();
            MMRProof::new(mmr_size, proof)
        };
        let digests_with_positions = {
            let result = content
                .headers()
                .into_iter()
                .map(|verifiable_header| {
                    let header = verifiable_header.header().into_view();
                    let index = header.number();
                    let position = leaf_index_to_pos(index);
                    let digest = header.digest();
                    digest.verify()?;
                    Ok((position, digest))
                })
                .collect::<Result<Vec<_>, String>>();
            assert!(result.is_ok(), "failed since {}", result.unwrap_err());
            result.unwrap()
        };
        let result = proof.verify(parent_chain_root, digests_with_positions);
        assert!(result.is_ok(), "failed since {}", result.unwrap_err());
    }

    assert_eq!(content.headers().len(), 1);

    let verifiable_header: VerifiableHeader = content.headers().get(0).unwrap().into();
    assert!(verifiable_header.header().is_genesis());
}
