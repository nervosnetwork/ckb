use crate::{
    LightClientProtocol,
    tests::{
        prelude::*,
        utils::{MockChain, MockNetworkContext},
    },
};
use ckb_app_config::{DBConfig, StoreConfig};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, leaf_index_to_pos};
use ckb_network::{CKBProtocolHandler, PeerIndex, SupportProtocols};
use ckb_shared::SharedBuilder;
use ckb_store::ChainStore;
use ckb_test_chain_utils::archive::{
    ArchiveFault, archive_and_collect, assert_cold, assert_hot, collect_published_archive,
    write_segmented_archive,
};
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, TransactionView, hardfork::HardForks},
    h256, packed,
    prelude::*,
    utilities::{
        MerkleProof,
        merkle_mountain_range::{MMRProof, VerifiableHeader},
        merkle_root,
    },
};
use std::{
    collections::HashSet,
    ops::RangeInclusive,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::Arc,
};

async fn request(protocol: &mut LightClientProtocol, message: packed::LightClientMessage) -> Bytes {
    let network = MockNetworkContext::new(SupportProtocols::LightClient);
    let peer = PeerIndex::new(1);
    protocol
        .received(network.context(), peer, message.as_bytes())
        .await;
    assert!(network.not_banned(peer));
    let messages = network.sent_messages().borrow();
    assert_eq!(messages.len(), 1);
    messages[0].2.clone()
}

/// Verify both MMR inclusion and the transaction/witness root against the hot corpus.
async fn check_proofs(
    protocol: &mut LightClientProtocol,
    corpus: &ProofCorpus,
    last: &packed::VerifiableHeader,
) -> Vec<Bytes> {
    let ProofCorpus {
        blocks,
        transaction,
        side,
    } = corpus;
    let absent = h256!("0x1234");
    let mut hashes: Vec<_> = blocks.iter().map(BlockView::hash).collect();
    hashes.extend([side.hash(), absent.clone().into()]);
    let response = request(
        protocol,
        packed::LightClientMessage::new_builder()
            .set(
                packed::GetBlocksProof::new_builder()
                    .last_hash(last.header().calc_header_hash())
                    .block_hashes(hashes)
                    .build(),
            )
            .build(),
    )
    .await;
    let packed::LightClientMessageUnion::SendBlocksProof(content) =
        packed::LightClientMessage::from_compatible_slice(&response)
            .unwrap()
            .to_enum()
    else {
        panic!("expected block proof")
    };
    let content = packed::SendBlocksProofV1::from_slice(content.as_slice()).unwrap();
    assert_eq!(content.last_header().as_slice(), last.as_slice());
    assert!(VerifiableHeader::from(content.last_header()).is_valid(0));
    assert_eq!(content.headers().len(), blocks.len());
    assert_eq!(
        content
            .missing_block_hashes()
            .into_iter()
            .collect::<HashSet<_>>(),
        HashSet::from([side.hash(), absent.clone().into()])
    );
    let headers: Vec<_> = content
        .headers()
        .into_iter()
        .map(|header| header.into_view())
        .collect();
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!(headers[index], block.header());
        assert_eq!(
            content.blocks_uncles_hash().get(index).unwrap(),
            block.calc_uncles_hash()
        );
        assert_eq!(
            content.blocks_extension().get(index).unwrap().to_opt(),
            block.extension()
        );
    }
    let root = last.parent_chain_root();
    let size = leaf_index_to_mmr_size(root.end_number().into());
    let leaves = headers
        .iter()
        .map(|header| (leaf_index_to_pos(header.number()), header.digest()))
        .collect();
    assert!(
        MMRProof::new(size, content.proof().into_iter().collect())
            .verify(root.clone(), leaves)
            .unwrap()
    );

    let tx_response = request(
        protocol,
        packed::LightClientMessage::new_builder()
            .set(
                packed::GetTransactionsProof::new_builder()
                    .last_hash(last.header().calc_header_hash())
                    .tx_hashes(vec![transaction.hash(), absent.clone().into()])
                    .build(),
            )
            .build(),
    )
    .await;
    let packed::LightClientMessageUnion::SendTransactionsProof(content) =
        packed::LightClientMessage::from_compatible_slice(&tx_response)
            .unwrap()
            .to_enum()
    else {
        panic!("expected transaction proof")
    };
    let content = packed::SendTransactionsProofV1::from_slice(content.as_slice()).unwrap();
    assert_eq!(content.last_header().as_slice(), last.as_slice());
    assert_eq!(
        content.missing_tx_hashes().into_iter().collect::<Vec<_>>(),
        vec![packed::Byte32::from(absent)]
    );
    assert_eq!(content.filtered_blocks().len(), 1);
    let filtered = content.filtered_blocks().get(0).unwrap();
    let header = filtered.header().into_view();
    let block = blocks
        .iter()
        .find(|block| block.hash() == header.hash())
        .unwrap();
    assert_eq!(
        content.blocks_extension().get(0).unwrap().to_opt(),
        block.extension()
    );
    assert_eq!(
        content.blocks_uncles_hash().get(0).unwrap(),
        block.calc_uncles_hash()
    );
    assert_eq!(
        filtered
            .transactions()
            .into_iter()
            .map(|tx| tx.into_view())
            .collect::<Vec<_>>(),
        vec![transaction.clone()]
    );
    let proof = filtered.proof();
    let proof = MerkleProof::new(
        proof.indices().into_iter().map(u32::from).collect(),
        proof.lemmas().into_iter().collect(),
    );
    assert!(
        !proof.lemmas().is_empty(),
        "exercise a nontrivial transaction proof"
    );
    let tx_root = proof.root(&[transaction.hash()]).unwrap();
    assert_eq!(filtered.witnesses_root(), block.calc_witnesses_root());
    assert_eq!(
        merkle_root(&[tx_root, filtered.witnesses_root()]),
        header.transactions_root()
    );
    assert!(
        MMRProof::new(size, content.proof().into_iter().collect())
            .verify(
                root,
                vec![(leaf_index_to_pos(header.number()), header.digest())]
            )
            .unwrap()
    );
    vec![response, tx_response]
}

struct ProofCorpus {
    blocks: Vec<BlockView>,
    transaction: TransactionView,
    side: BlockView,
}

impl ProofCorpus {
    fn mine(chain: &MockChain) -> Self {
        chain.mine_to(20);
        let transaction = chain.get_cellbase_as_input(12);
        let second = chain.get_cellbase_as_input(13);
        chain.mine_block(|block| {
            block
                .as_advanced_builder()
                .proposals(vec![
                    transaction.proposal_short_id(),
                    second.proposal_short_id(),
                ])
                .build()
        });
        chain.mine_blocks(1);
        let committed = chain.mine_block(|block| {
            block
                .as_advanced_builder()
                .transaction(transaction.clone())
                .transaction(second.clone())
                .build()
        });
        chain.mine_to(25);
        let side = chain.shared().snapshot().get_block_by_number(25).unwrap();
        chain.rollback_to(24, HashSet::new());
        chain.mine_block(|block| block.as_advanced_builder().nonce(1u128).build());
        chain.mine_blocks(1);
        let snapshot = chain.shared().cloned_snapshot();
        let blocks: Vec<_> = [0, 6, committed, 25]
            .into_iter()
            .map(|number| snapshot.get_block_by_number(number).unwrap())
            .collect();
        assert!(blocks[0].extension().is_none());
        assert!(blocks[1].extension().is_some());
        assert_ne!(blocks[3].hash(), side.hash());
        Self {
            blocks,
            transaction,
            side,
        }
    }

    fn archive_blocks(&self, chain: &MockChain) -> Vec<BlockView> {
        let snapshot = chain.shared().snapshot();
        let mut blocks = self.blocks.clone();
        blocks.extend([
            self.side.clone(),
            snapshot.get_block_by_number(14).unwrap(),
            snapshot.get_block_by_number(snapshot.tip_number()).unwrap(),
        ]);
        blocks
    }
}

fn persistent_builder(
    directory: &Path,
    consensus: Consensus,
    archive: Option<PathBuf>,
) -> SharedBuilder {
    std::fs::create_dir_all(directory).unwrap();
    let freezer_enable = archive.is_some();
    SharedBuilder::new(
        "archive-consumer-test",
        directory,
        &DBConfig {
            path: directory.join("db"),
            ..Default::default()
        },
        archive,
        Handle::new(tokio::runtime::Handle::current(), None),
        consensus,
    )
    .unwrap()
    .store_config(StoreConfig {
        freezer_enable,
        ..Default::default()
    })
}

fn replay_blocks(destination: &MockChain, source: &MockChain, numbers: RangeInclusive<u64>) {
    for number in numbers {
        destination.process_block(
            source
                .shared()
                .snapshot()
                .get_block_by_number(number)
                .unwrap(),
        );
    }
}

#[test]
fn archived_block_and_transaction_proofs_survive_collection_and_reopen() {
    exercise_archived_proofs(false);
}

#[test]
fn epoch_archive_proofs_survive_whole_segment_drop_and_reopen() {
    exercise_archived_proofs(true);
}

fn exercise_archived_proofs(segmented: bool) {
    let directory = tempfile::tempdir().unwrap();
    let cold_path = directory.path().join("cold");
    let hot_path = directory.path().join("hot");
    let (consensus, mut corpus, archived, last, expected) = {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let cold = MockChain::with_builder(|mut consensus| {
                consensus.hardfork_switch = HardForks::new_dev();
                persistent_builder(&cold_path, consensus, Some(cold_path.join("archive")))
                    .store_config(StoreConfig {
                        freezer_enable: true,
                        freezer_epoch_segments: segmented,
                        ..Default::default()
                    })
            });
            assert_eq!(
                cold.shared()
                    .store()
                    .db()
                    .epoch_segments()
                    .active()
                    .is_some(),
                segmented
            );
            let corpus = ProofCorpus::mine(&cold);
            let consensus = cold.shared().consensus().clone();
            let hot =
                MockChain::with_builder(|_| persistent_builder(&hot_path, consensus.clone(), None));
            // Replay the exact blocks through an independent chain, including the old fork.
            replay_blocks(&hot, &cold, 1..=24);
            hot.process_block(corpus.side.clone());
            hot.rollback_to(24, HashSet::new());
            replay_blocks(&hot, &cold, 25..=26);
            let archived = corpus.archive_blocks(&cold);
            assert_hot(hot.shared().store(), &archived);

            let before = cold.shared().cloned_snapshot();
            let last = before
                .get_verifiable_header_by_number(before.tip_number())
                .unwrap();
            let mut cold_protocol = cold.create_light_client_protocol();
            let mut hot_protocol = hot.create_light_client_protocol();
            let expected = check_proofs(&mut hot_protocol, &corpus, &last).await;
            assert_eq!(
                check_proofs(&mut cold_protocol, &corpus, &last).await,
                expected
            );
            archive_and_collect(cold.shared().store(), &archived).await;
            cold.shared().refresh_snapshot();
            assert_eq!(
                check_proofs(&mut cold_protocol, &corpus, &last).await,
                expected
            );
            cold.shared().store_snapshot(before);
            assert_eq!(
                check_proofs(&mut cold_protocol, &corpus, &last).await,
                expected
            );
            cold.shared().refresh_snapshot();

            // Both stores validate the same later spend of an archived cellbase.
            let following = cold.get_cellbase_as_input(14);
            cold.mine_block(|block| {
                block
                    .as_advanced_builder()
                    .proposal(following.proposal_short_id())
                    .build()
            });
            cold.mine_blocks(1);
            cold.mine_block(|block| {
                block
                    .as_advanced_builder()
                    .transaction(following.clone())
                    .build()
            });
            replay_blocks(&hot, &cold, 27..=29);
            for chain in [&cold, &hot] {
                assert_eq!(
                    chain
                        .shared()
                        .snapshot()
                        .get_transaction(&following.hash())
                        .unwrap()
                        .0,
                    following
                );
            }
            let last = cold
                .shared()
                .snapshot()
                .get_verifiable_header_by_number(29)
                .unwrap();
            let expected = check_proofs(&mut hot_protocol, &corpus, &last).await;
            assert_eq!(
                check_proofs(&mut cold_protocol, &corpus, &last).await,
                expected
            );
            assert_cold(cold.shared().store(), &archived);
            assert_hot(hot.shared().store(), &archived);
            (consensus, corpus, archived, last, expected)
        })
        // Drop both chains and their runtime before reopening either database.
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let cold = MockChain::with_builder(|_| {
            persistent_builder(
                &cold_path,
                consensus.clone(),
                Some(cold_path.join("archive")),
            )
        });
        let hot = MockChain::with_builder(|_| persistent_builder(&hot_path, consensus, None));
        assert_eq!(
            cold.shared()
                .store()
                .db()
                .epoch_segments()
                .active()
                .is_some(),
            segmented
        );
        assert_cold(cold.shared().store(), &archived);
        assert_hot(hot.shared().store(), &archived);
        let mut cold_protocol = cold.create_light_client_protocol();
        let mut hot_protocol = hot.create_light_client_protocol();
        assert_eq!(
            check_proofs(&mut cold_protocol, &corpus, &last).await,
            expected
        );
        assert_eq!(
            check_proofs(&mut hot_protocol, &corpus, &last).await,
            expected
        );

        let cold_before = cold.shared().cloned_snapshot();
        let hot_before = hot.shared().cloned_snapshot();
        for chain in [&cold, &hot] {
            chain.rollback_to(24, HashSet::new());
        }
        cold.mine_block(|block| block.as_advanced_builder().nonce(2u128).build());
        cold.mine_to(29);
        replay_blocks(&hot, &cold, 25..=29);
        let cold_after = cold.shared().cloned_snapshot();
        let hot_after = hot.shared().cloned_snapshot();
        cold.shared().store_snapshot(cold_before);
        hot.shared().store_snapshot(hot_before);
        assert_eq!(
            check_proofs(&mut cold_protocol, &corpus, &last).await,
            expected
        );
        assert_eq!(
            check_proofs(&mut hot_protocol, &corpus, &last).await,
            expected
        );
        cold.shared().store_snapshot(Arc::clone(&cold_after));
        hot.shared().store_snapshot(hot_after);

        let replacement = cold_after.get_block_by_number(25).unwrap();
        corpus.side = std::mem::replace(&mut corpus.blocks[3], replacement);
        let last = cold_after
            .get_verifiable_header_by_number(cold_after.tip_number())
            .unwrap();
        assert_eq!(
            check_proofs(&mut cold_protocol, &corpus, &last).await,
            check_proofs(&mut hot_protocol, &corpus, &last).await,
        );
        assert_cold(cold.shared().store(), &archived);
        assert_hot(hot.shared().store(), &archived);
    });
}

#[test]
fn archived_storage_faults_never_send_block_or_transaction_proofs() {
    let seed = tempfile::tempdir().unwrap();
    let (consensus, corpus, archived, last, expected) = {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let chain = MockChain::with_builder(|mut consensus| {
                consensus.hardfork_switch = HardForks::new_dev();
                persistent_builder(seed.path(), consensus, None)
            });
            let corpus = ProofCorpus::mine(&chain);
            // Keep the fault target out of startup's recent-block reads.
            chain.mine_to(40);
            let archived = corpus.archive_blocks(&chain);
            let snapshot = chain.shared().snapshot();
            let last = snapshot
                .get_verifiable_header_by_number(snapshot.tip_number())
                .unwrap();
            let expected =
                check_proofs(&mut chain.create_light_client_protocol(), &corpus, &last).await;
            (
                chain.shared().consensus().clone(),
                corpus,
                archived,
                last,
                expected,
            )
        })
    };
    write_segmented_archive(&seed.path().join("archive"), &archived);
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (shared, _pack) = persistent_builder(
                seed.path(),
                consensus.clone(),
                Some(seed.path().join("archive")),
            )
            .build()
            .unwrap();
            collect_published_archive(shared.store(), &archived);
            shared.refresh_snapshot();
            assert_eq!(
                check_proofs(&mut LightClientProtocol::new(shared), &corpus, &last).await,
                expected
            );
        });
    }
    // Every fault starts from a closed, valid archive with a fresh reader/cache.
    for fault in ArchiveFault::ALL {
        let directory = tempfile::tempdir().unwrap();
        copy_closed_fixture(seed.path(), directory.path());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let shared = runtime.block_on(async {
            persistent_builder(
                directory.path(),
                consensus.clone(),
                Some(directory.path().join("archive")),
            )
            .build()
            .unwrap()
            .0
        });
        assert_cold(shared.store(), &archived);
        let target = &corpus.blocks[3];
        assert_eq!(target.number(), corpus.side.number());
        fault.inject(
            shared.store(),
            &directory.path().join("archive"),
            target,
            &corpus.side,
        );
        shared.refresh_snapshot();
        let mut protocol = LightClientProtocol::new(shared);
        let requests = [
            packed::LightClientMessage::new_builder()
                .set(
                    packed::GetBlocksProof::new_builder()
                        .last_hash(last.header().calc_header_hash())
                        .block_hashes(vec![target.hash()])
                        .build(),
                )
                .build(),
            packed::LightClientMessage::new_builder()
                .set(
                    packed::GetTransactionsProof::new_builder()
                        .last_hash(last.header().calc_header_hash())
                        .tx_hashes(vec![target.transactions()[0].hash()])
                        .build(),
                )
                .build(),
        ];
        for message in requests {
            let network = MockNetworkContext::new(SupportProtocols::LightClient);
            let peer = PeerIndex::new(1);
            // Do not catch assertions from the response checker as storage failures.
            let result = catch_unwind(AssertUnwindSafe(|| {
                runtime.block_on(protocol.received(network.context(), peer, message.as_bytes()))
            }));
            fault.assert_failure(
                result.expect_err("storage fault returned a normal protocol result"),
            );
            assert!(
                network.sent_messages().borrow().is_empty(),
                "{fault:?} sent a response"
            );
            assert!(network.not_banned(peer), "{fault:?} blamed the peer");
        }
    }
}

fn copy_closed_fixture(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_closed_fixture(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
