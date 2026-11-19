use crate::{
    Node, Spec,
    node::waiting_for_sync,
    util::{check::is_transaction_pending, log_monitor::monitor_log_until_expected_show},
    utils::wait_until,
};
use ckb_chain_spec::HardForkConfig;
use ckb_db::internal::{DB, Options};
use ckb_db_schema::{COLUMN_BLOCK_EXTENSION, freezer::segment_name};
use ckb_jsonrpc_types::Status;
use ckb_store::ChainStore;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockView, Capacity, ScriptHashType, TransactionBuilder, TransactionView, capacity_bytes,
    },
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};
use std::{fs, time::Instant};

/// Relay a VM v2 read of an archived block after its epoch column has been retired.
pub struct FreezerBlockExtensionRelay;

impl Spec for FreezerBlockExtensionRelay {
    fn before_run(&self) -> Vec<Node> {
        let nonce = rand::random::<u128>();
        [("hot", false), ("freezer", true)]
            .into_iter()
            .map(|(name, enabled)| {
                let mut node = Node::new(self.name(), name);
                node.modify_chain_spec(|spec| {
                    spec.genesis.nonce = nonce.into();
                    spec.params.genesis_epoch_length = Some(2);
                    spec.params.epoch_duration_target = Some(16);
                    spec.params.permanent_difficulty_in_dummy = Some(true);
                    spec.params.hardfork = Some(HardForkConfig { ckb2023: Some(0) });
                });
                node.modify_app_config(|config| {
                    config.store.freezer_enable = enabled;
                    config.store.freezer_epoch_segments = enabled;
                    config.db.freezer_epoch_width = Some(8);
                    config.network.whitelist_only = false;
                    config.network.whitelist_peers.clear();
                    config.network.sync.assume_valid_targets = Some(vec![Default::default()]);
                    config.logger.filter = Some("info,ckb_script=debug".into());
                    config.logger.color = false;
                });
                node.start();
                node
            })
            .collect()
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        let [hot, freezer] = nodes.as_mut_slice() else {
            panic!("expected two nodes")
        };
        let target = archive_and_reopen(hot, freezer);
        let extension = target.extension().expect("CKB2023 chain root");
        let (hot, freezer) = (&*hot, &*freezer);

        let tx = prepare_observed_transfer(hot, &target);
        waiting_for_sync(&[hot, freezer]);
        let monitoring_since = Instant::now();
        check_peers(hot, freezer, monitoring_since);
        let offsets = [hot, freezer].map(|node| fs::metadata(node.log_path()).unwrap().len());
        assert_eq!(hot.submit_transaction(&tx), tx.hash());
        let expected = format!("Freezer extension {:x}", extension.raw_data());
        for (node, offset) in [hot, freezer].into_iter().zip(offsets) {
            assert!(
                wait_until(60, || is_transaction_pending(node, &tx)),
                "transaction did not enter both pools"
            );
            let observation = monitor_log_until_expected_show(
                node,
                offset,
                20,
                "DEBUG OUTPUT: Freezer extension ",
            )
            .expect("missing transaction VM observation");
            assert!(
                observation.trim_end().ends_with(&expected),
                "unexpected VM observation: {observation}"
            );
        }
        let cycles = hot.rpc_client().get_transaction(tx.hash()).cycles.unwrap();
        assert_eq!(
            freezer.rpc_client().get_transaction(tx.hash()).cycles,
            Some(cycles)
        );
        check_peers(hot, freezer, monitoring_since);

        hot.mine_until_transaction_confirm(&tx.hash());
        waiting_for_sync(&[hot, freezer]);
        let included = hot.get_tip_block();
        assert!(
            included
                .transactions()
                .iter()
                .any(|actual| actual.data().as_slice() == tx.data().as_slice())
        );
        assert_eq!(
            freezer
                .get_block_by_number(included.number())
                .data()
                .as_slice(),
            included.data().as_slice()
        );
        for node in [hot, freezer] {
            let status = node.get_transaction(tx.hash());
            assert_eq!(status.status, Status::Committed);
            assert_eq!(status.block_hash, Some(included.hash().into()));
        }
        check_peers(hot, freezer, monitoring_since);

        freezer.mine(1);
        waiting_for_sync(&[hot, freezer]);
        let child = freezer.get_tip_block();
        assert_eq!(child.parent_hash(), included.hash());
        assert_eq!(hot.get_tip_block().hash(), child.hash());
        check_peers(hot, freezer, monitoring_since);
    }
}

fn archive_and_reopen(hot: &mut Node, freezer: &mut Node) -> BlockView {
    hot.mine(2);
    freezer.connect(hot);
    waiting_for_sync(&[&*hot, &*freezer]);
    let target = hot.get_block_by_number(2);
    let hash = target.hash();
    let extension = target.extension().expect("CKB2023 chain root");
    let segment = segment_name(0);
    let db_path = freezer.working_dir().join("data/db");
    let column_families = || DB::list_cf(&Options::default(), &db_path);
    assert!(column_families().unwrap().contains(&segment));

    // Age past the normal 100-epoch retention window. Small RPC batches
    // leave headroom for slower CI runners.
    for _ in 0..14 {
        hot.rpc_client()
            .inner()
            .generate_epochs(8.into())
            .expect("mine eight epochs");
    }
    waiting_for_sync(&[&*hot, &*freezer]);
    assert!(
        wait_until(240, || column_families()
            .is_ok_and(|cfs| !cfs.contains(&segment))),
        "the Freezer worker did not retire the target epoch column"
    );

    // Check both persisted routes, then reopen with empty node caches.
    freezer.stop_gracefully();
    hot.stop_gracefully();
    freezer.access_db(|store| {
        let archived = store
            .get_archived_block(&hash)
            .expect("committed archive record");
        assert_eq!(archived.as_slice(), target.data().as_slice());
        assert!(store.get(COLUMN_BLOCK_EXTENSION, hash.as_slice()).is_none());
        assert!(store.get_hot_block_body(&hash).is_empty());
    });
    hot.access_db(|store| {
        assert!(store.get_archived_block(&hash).is_none());
        assert_eq!(
            store
                .get(COLUMN_BLOCK_EXTENSION, hash.as_slice())
                .unwrap()
                .as_ref(),
            extension.as_slice()
        );
    });
    hot.start();
    freezer.start();
    freezer.connect(hot);
    waiting_for_sync(&[&*hot, &*freezer]);
    target
}

fn prepare_observed_transfer(node: &Node, target: &BlockView) -> TransactionView {
    let genesis = node.consensus().genesis_block();
    let mut code = include_bytes!("../../../../script/testdata/observe_freezer_extension").to_vec();
    code.extend_from_slice(genesis.data().header().as_slice());
    let code = Bytes::from(code);
    let observer = Script::new_builder()
        .code_hash(CellOutput::calc_data_hash(&code))
        .hash_type(ScriptHashType::Data2)
        .build();
    let code_cell = CellOutput::new_builder()
        .lock(node.always_success_script())
        .build_exact_capacity(Capacity::bytes(code.len()).unwrap())
        .unwrap();
    let funding = node.get_tip_block().transactions()[0].hash();
    let setup = TransactionBuilder::default()
        .cell_dep(node.always_success_cell_dep())
        .input(CellInput::new(OutPoint::new(funding, 0), 0))
        .output(code_cell)
        .output_data(code)
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(200))
                .lock(observer)
                .build(),
        )
        .output_data(Bytes::new())
        .build();
    node.submit_transaction(&setup);
    node.mine_until_transaction_confirm(&setup.hash());

    TransactionBuilder::default()
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(setup.hash(), 0))
                .build(),
        )
        .header_dep(genesis.hash())
        .header_dep(target.hash())
        .input(CellInput::new(OutPoint::new(setup.hash(), 1), 0))
        .output(
            CellOutput::new_builder()
                .capacity(capacity_bytes!(199))
                .lock(node.always_success_script())
                .build(),
        )
        .output_data(Bytes::new())
        .build()
}

fn check_peers(hot: &Node, freezer: &Node, since: Instant) {
    for (node, other) in [(hot, freezer), (freezer, hot)] {
        assert!(node.rpc_client().get_banned_addresses().is_empty());
        let elapsed_ms = since.elapsed().as_millis();
        let peers = node.rpc_client().get_peers();
        let [peer] = peers.as_slice() else {
            panic!("expected exactly one connected peer")
        };
        assert_eq!(peer.node_id, other.node_id());
        // A reconnect between samples resets the session age.
        assert!(
            u128::from(peer.connected_duration.value()) >= elapsed_ms,
            "peer reconnected during relay"
        );
    }
}
