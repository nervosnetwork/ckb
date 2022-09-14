use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_constant::sync::{CHAIN_SYNC_TIMEOUT, EVICTION_HEADERS_RESPONSE_TIME, MAX_TIP_AGE};
use ckb_dao::DaoCalculator;
use ckb_error::InternalErrorKind;
use ckb_launcher::SharedBuilder;
use ckb_network::{
    async_trait, bytes::Bytes, Behaviour, CKBProtocolContext, Peer, PeerId, PeerIndex, ProtocolId,
    SessionType, TargetSession,
};
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::{Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::resolve_transaction, BlockBuilder, BlockNumber, BlockView, EpochExt, HeaderBuilder,
        HeaderView as CoreHeaderView, TransactionBuilder, TransactionView,
    },
    packed::{
        self, Byte32, CellInput, CellOutputBuilder, Script, SendBlockBuilder, SendHeadersBuilder,
    },
    prelude::*,
    utilities::difficulty_to_compact,
    U256,
};
use ckb_util::Mutex;
use ckb_verification_traits::Switch;
use faketime::unix_time_as_millis;
use futures::future::Future;
use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    pin::Pin,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use crate::{
    synchronizer::{BlockFetcher, BlockProcess, GetBlocksProcess, HeadersProcess, Synchronizer},
    types::{HeaderView, HeadersSyncController, IBDState, PeerState},
    Status, StatusCode, SyncShared,
};

fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared, Synchronizer) {
    let mut builder = SharedBuilder::with_temp_db();

    let consensus = consensus.unwrap_or_default();
    builder = builder.consensus(consensus);

    let (shared, mut pack) = builder.build().unwrap();

    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let chain_controller = chain_service.start::<&str>(None);

    let sync_shared = Arc::new(SyncShared::new(
        shared.clone(),
        Default::default(),
        pack.take_relay_tx_receiver(),
    ));
    let synchronizer = Synchronizer::new(chain_controller.clone(), sync_shared);

    (chain_controller, shared, synchronizer)
}

fn create_cellbase(
    shared: &Shared,
    parent_header: &CoreHeaderView,
    number: BlockNumber,
) -> TransactionView {
    let (_, reward) = RewardCalculator::new(shared.consensus(), shared.snapshot().as_ref())
        .block_reward_to_finalize(parent_header)
        .unwrap();

    let builder = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .witness(Script::default().into_witness());
    if number <= shared.consensus().finalization_delay_length() {
        builder.build()
    } else {
        builder
            .output(
                CellOutputBuilder::default()
                    .capacity(reward.total.pack())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build()
    }
}

fn gen_block(
    shared: &Shared,
    parent_header: &CoreHeaderView,
    epoch: &EpochExt,
    nonce: u128,
) -> BlockView {
    let now = 1 + parent_header.timestamp();
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(shared, parent_header, number);
    let dao = {
        let snapshot: &Snapshot = &shared.snapshot();
        let resolved_cellbase =
            resolve_transaction(cellbase.clone(), &mut HashSet::new(), snapshot, snapshot).unwrap();
        let data_loader = shared.store().borrow_as_data_loader();
        DaoCalculator::new(shared.consensus(), &data_loader)
            .dao_field(&[resolved_cellbase], parent_header)
            .unwrap()
    };

    BlockBuilder::default()
        .transaction(cellbase)
        .parent_hash(parent_header.hash())
        .timestamp(now.pack())
        .epoch(epoch.number_with_fraction(number).pack())
        .number(number.pack())
        .compact_target(epoch.compact_target().pack())
        .nonce(nonce.pack())
        .dao(dao)
        .build()
}

fn insert_block(
    chain_controller: &ChainController,
    shared: &Shared,
    nonce: u128,
    number: BlockNumber,
) {
    let snapshot = shared.snapshot();
    let parent = snapshot
        .get_block_header(&snapshot.get_block_hash(number - 1).unwrap())
        .unwrap();
    let epoch = snapshot
        .consensus()
        .next_epoch_ext(&parent, &snapshot.borrow_as_data_loader())
        .unwrap()
        .epoch();

    let block = gen_block(shared, &parent, &epoch, nonce);

    chain_controller
        .process_block(Arc::new(block))
        .expect("process block ok");
}

#[test]
fn test_locator() {
    let (chain_controller, shared, synchronizer) = start_chain(None);

    let num = 200;
    let index = [
        199, 198, 197, 196, 195, 194, 193, 192, 191, 190, 188, 184, 176, 160, 128, 64,
    ];

    for i in 1..num {
        insert_block(&chain_controller, &shared, u128::from(i), i);
    }

    let locator = synchronizer
        .shared
        .active_chain()
        .get_locator(shared.snapshot().tip_header());

    let mut expect = Vec::new();

    for i in index.iter() {
        expect.push(shared.store().get_block_hash(*i).unwrap());
    }
    //genesis_hash must be the last one
    expect.push(shared.genesis_hash());

    assert_eq!(expect, locator);
}

#[test]
fn test_locate_latest_common_block() {
    let consensus = Consensus::default();
    let (chain_controller1, shared1, synchronizer1) = start_chain(Some(consensus.clone()));
    let (chain_controller2, shared2, synchronizer2) = start_chain(Some(consensus.clone()));
    let num = 200;

    for i in 1..num {
        insert_block(&chain_controller1, &shared1, u128::from(i), i);
    }

    for i in 1..num {
        insert_block(&chain_controller2, &shared2, u128::from(i + 1), i);
    }

    let locator1 = synchronizer1
        .shared
        .active_chain()
        .get_locator(shared1.snapshot().tip_header());

    let latest_common = synchronizer2
        .shared
        .active_chain()
        .locate_latest_common_block(&Byte32::zero(), &locator1[..]);

    assert_eq!(latest_common, Some(0));

    let (chain_controller3, shared3, synchronizer3) = start_chain(Some(consensus));

    for i in 1..num {
        let j = if i > 192 { i + 1 } else { i };
        insert_block(&chain_controller3, &shared3, u128::from(j), i);
    }

    let latest_common3 = synchronizer3
        .shared
        .active_chain()
        .locate_latest_common_block(&Byte32::zero(), &locator1[..]);
    assert_eq!(latest_common3, Some(192));
}

#[test]
fn test_locate_latest_common_block2() {
    let consensus = Consensus::default();
    let (chain_controller1, shared1, synchronizer1) = start_chain(Some(consensus.clone()));
    let (chain_controller2, shared2, synchronizer2) = start_chain(Some(consensus.clone()));
    let block_number = 200;

    let mut blocks: Vec<BlockView> = Vec::new();
    let mut parent = consensus.genesis_block().header();

    for i in 1..block_number {
        let store = shared1.store();
        let epoch = shared1
            .consensus()
            .next_epoch_ext(&parent, &store.borrow_as_data_loader())
            .unwrap()
            .epoch();
        let new_block = gen_block(&shared1, &parent, &epoch, i);
        blocks.push(new_block.clone());

        chain_controller1
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        chain_controller2
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        parent = new_block.header().to_owned();
    }

    parent = blocks[150].header();
    let fork = parent.number();
    for i in 1..=block_number {
        let store = shared2.store();
        let epoch = shared2
            .consensus()
            .next_epoch_ext(&parent, &store.borrow_as_data_loader())
            .unwrap()
            .epoch();
        let new_block = gen_block(&shared2, &parent, &epoch, i + 100);

        chain_controller2
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        parent = new_block.header().to_owned();
    }

    let locator1 = synchronizer1
        .shared
        .active_chain()
        .get_locator(shared1.snapshot().tip_header());

    let latest_common = synchronizer2
        .shared
        .active_chain()
        .locate_latest_common_block(&Byte32::zero(), &locator1[..])
        .unwrap();

    assert_eq!(
        shared1.snapshot().get_block_hash(fork).unwrap(),
        shared2.snapshot().get_block_hash(fork).unwrap()
    );
    assert!(
        shared1.snapshot().get_block_hash(fork + 1).unwrap()
            != shared2.snapshot().get_block_hash(fork + 1).unwrap()
    );
    assert_eq!(
        shared1.snapshot().get_block_hash(latest_common).unwrap(),
        shared1.snapshot().get_block_hash(fork).unwrap()
    );
}

#[test]
fn test_get_ancestor() {
    let consensus = Consensus::default();
    let (chain_controller, shared, synchronizer) = start_chain(Some(consensus));
    let num = 200;

    for i in 1..num {
        insert_block(&chain_controller, &shared, u128::from(i), i);
    }

    let header = synchronizer
        .shared
        .active_chain()
        .get_ancestor(&shared.snapshot().tip_header().hash(), 100);
    let tip = synchronizer
        .shared
        .active_chain()
        .get_ancestor(&shared.snapshot().tip_header().hash(), 199);
    let noop = synchronizer
        .shared
        .active_chain()
        .get_ancestor(&shared.snapshot().tip_header().hash(), 200);
    assert!(tip.is_some());
    assert!(header.is_some());
    assert!(noop.is_none());
    assert_eq!(tip.unwrap(), shared.snapshot().tip_header().to_owned());
    assert_eq!(
        header.unwrap(),
        shared
            .store()
            .get_block_header(&shared.store().get_block_hash(100).unwrap())
            .unwrap()
    );
}

#[test]
fn test_process_new_block() {
    let consensus = Consensus::default();
    let (chain_controller1, shared1, _) = start_chain(Some(consensus.clone()));
    let (_, shared2, synchronizer) = start_chain(Some(consensus));
    let block_number = 2000;

    let mut blocks: Vec<BlockView> = Vec::new();
    let mut parent = shared1
        .store()
        .get_block_header(&shared1.store().get_block_hash(0).unwrap())
        .unwrap();
    for i in 1..block_number {
        let store = shared1.store();
        let epoch = shared1
            .consensus()
            .next_epoch_ext(&parent, &store.borrow_as_data_loader())
            .unwrap()
            .epoch();
        let new_block = gen_block(&shared1, &parent, &epoch, i + 100);

        chain_controller1
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        parent = new_block.header().to_owned();
        blocks.push(new_block);
    }
    let chain1_last_block = blocks.last().cloned().unwrap();
    blocks.into_iter().for_each(|block| {
        synchronizer
            .shared()
            .insert_new_block(&synchronizer.chain, Arc::new(block))
            .expect("Insert new block failed");
    });
    assert_eq!(&chain1_last_block.header(), shared2.snapshot().tip_header());
}

#[test]
fn test_get_locator_response() {
    let consensus = Consensus::default();
    let (chain_controller, shared, synchronizer) = start_chain(Some(consensus));
    let block_number = 200;

    let mut blocks: Vec<BlockView> = Vec::new();
    let mut parent = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    for i in 1..=block_number {
        let store = shared.snapshot();
        let epoch = shared
            .consensus()
            .next_epoch_ext(&parent, &store.borrow_as_data_loader())
            .unwrap()
            .epoch();
        let new_block = gen_block(&shared, &parent, &epoch, i + 100);
        blocks.push(new_block.clone());

        chain_controller
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        parent = new_block.header().to_owned();
    }

    let headers = synchronizer
        .shared
        .active_chain()
        .get_locator_response(180, &Byte32::zero());

    assert_eq!(headers.first().unwrap(), &blocks[180].header());
    assert_eq!(headers.last().unwrap(), &blocks[199].header());

    for window in headers.windows(2) {
        if let [parent, header] = &window {
            assert_eq!(header.data().raw().parent_hash(), parent.hash());
        }
    }
}

#[derive(Clone)]
struct DummyNetworkContext {
    pub peers: HashMap<PeerIndex, Peer>,
    pub disconnected: Arc<Mutex<HashSet<PeerIndex>>>,
}

fn mock_peer_info() -> Peer {
    Peer::new(
        0.into(),
        SessionType::Outbound,
        format!("/ip4/127.0.0.1/tcp/42/p2p/{}", PeerId::random().to_base58())
            .parse()
            .expect("parse multiaddr"),
        false,
    )
}

fn mock_header_view(total_difficulty: u64) -> HeaderView {
    HeaderView::new(
        HeaderBuilder::default().build(),
        U256::from(total_difficulty),
    )
}

#[async_trait]
impl CKBProtocolContext for DummyNetworkContext {
    // Interact with underlying p2p service
    async fn set_notify(&self, _interval: Duration, _token: u64) -> Result<(), ckb_network::Error> {
        unimplemented!();
    }

    async fn remove_notify(&self, _token: u64) -> Result<(), ckb_network::Error> {
        unimplemented!()
    }
    async fn async_future_task(
        &self,
        _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
        _blocking: bool,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }

    async fn async_quick_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.send_message(proto_id, peer_index, data)
    }
    async fn async_quick_send_message_to(
        &self,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.send_message_to(peer_index, data)
    }
    async fn async_quick_filter_broadcast(
        &self,
        target: TargetSession,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.filter_broadcast(target, data)
    }
    async fn async_send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    async fn async_send_message_to(
        &self,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    async fn async_filter_broadcast(
        &self,
        _target: TargetSession,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    async fn async_disconnect(
        &self,
        peer_index: PeerIndex,
        _msg: &str,
    ) -> Result<(), ckb_network::Error> {
        self.disconnected.lock().insert(peer_index);
        Ok(())
    }

    fn future_task(
        &self,
        _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
        _blocking: bool,
    ) -> Result<(), ckb_network::Error> {
        //            task.await.expect("resolve future task error");
        Ok(())
    }

    fn quick_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.send_message(proto_id, peer_index, data)
    }
    fn quick_send_message_to(
        &self,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.send_message_to(peer_index, data)
    }
    fn quick_filter_broadcast(
        &self,
        target: TargetSession,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.filter_broadcast(target, data)
    }
    fn send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    fn send_message_to(
        &self,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    fn filter_broadcast(
        &self,
        _target: TargetSession,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    fn disconnect(&self, peer_index: PeerIndex, _msg: &str) -> Result<(), ckb_network::Error> {
        self.disconnected.lock().insert(peer_index);
        Ok(())
    }
    // Interact with NetworkState
    fn get_peer(&self, peer_index: PeerIndex) -> Option<Peer> {
        self.peers.get(&peer_index).cloned()
    }
    fn with_peer_mut(&self, _peer_index: PeerIndex, _f: Box<dyn FnOnce(&mut Peer)>) {}
    fn connected_peers(&self) -> Vec<PeerIndex> {
        unimplemented!();
    }
    fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {}
    fn ban_peer(&self, _peer_index: PeerIndex, _duration: Duration, _reason: String) {}
    // Other methods
    fn protocol_id(&self) -> ProtocolId {
        ProtocolId::new(1)
    }
}

fn mock_network_context(peer_num: usize) -> DummyNetworkContext {
    let mut peers = HashMap::default();
    for peer in 0..peer_num {
        peers.insert(peer.into(), mock_peer_info());
    }
    DummyNetworkContext {
        peers,
        disconnected: Arc::new(Mutex::new(HashSet::default())),
    }
}

#[test]
fn test_sync_process() {
    let consensus = Consensus::default();
    let (chain_controller1, shared1, synchronizer1) = start_chain(Some(consensus.clone()));
    let (chain_controller2, shared2, synchronizer2) = start_chain(Some(consensus));
    let num = 200;

    for i in 1..num {
        insert_block(&chain_controller1, &shared1, u128::from(i), i);
    }

    let locator1 = synchronizer1
        .shared
        .active_chain()
        .get_locator(shared1.snapshot().tip_header());

    for i in 1..=num {
        let j = if i > 192 { i + 1 } else { i };
        insert_block(&chain_controller2, &shared2, u128::from(j), i);
    }

    let latest_common = synchronizer2
        .shared
        .active_chain()
        .locate_latest_common_block(&Byte32::zero(), &locator1[..]);
    assert_eq!(latest_common, Some(192));

    let headers = synchronizer2
        .shared
        .active_chain()
        .get_locator_response(192, &Byte32::zero());

    assert_eq!(
        headers.first().unwrap().hash(),
        shared2.store().get_block_hash(193).unwrap()
    );
    assert_eq!(
        headers.last().unwrap().hash(),
        shared2.store().get_block_hash(200).unwrap()
    );

    let sendheaders = SendHeadersBuilder::default()
        .headers(headers.iter().map(|h| h.data()).pack())
        .build();

    let mock_nc = mock_network_context(4);
    let peer1: PeerIndex = 1.into();
    let peer2: PeerIndex = 2.into();
    synchronizer1.on_connected(&mock_nc, peer1);
    synchronizer1.on_connected(&mock_nc, peer2);
    assert_eq!(
        HeadersProcess::new(sendheaders.as_reader(), &synchronizer1, peer1, &mock_nc).execute(),
        Status::ok(),
    );

    let best_known_header = synchronizer1.peers().get_best_known_header(peer1);

    assert_eq!(best_known_header.unwrap().inner(), headers.last().unwrap());

    let blocks_to_fetch = synchronizer1
        .get_blocks_to_fetch(peer1, IBDState::Out)
        .unwrap();

    assert_eq!(
        blocks_to_fetch[0].first().unwrap(),
        &shared2.store().get_block_hash(193).unwrap()
    );
    assert_eq!(
        blocks_to_fetch[0].last().unwrap(),
        &shared2.store().get_block_hash(200).unwrap()
    );

    let mut fetched_blocks = Vec::new();
    for block_hash in &blocks_to_fetch[0] {
        fetched_blocks.push(shared2.store().get_block(block_hash).unwrap());
    }

    for block in &fetched_blocks {
        let block = SendBlockBuilder::default().block(block.data()).build();
        assert_eq!(
            BlockProcess::new(block.as_reader(), &synchronizer1, peer1).execute(),
            Status::ok(),
        );
    }

    // After the above blocks stored, we should remove them from in-flight pool
    synchronizer1
        .shared()
        .state()
        .write_inflight_blocks()
        .remove_by_peer(peer1);

    // Construct a better tip, to trigger fixing last_common_header inside `get_blocks_to_fetch`
    insert_block(&synchronizer2.chain, &shared2, 201u128, 201);
    let headers = vec![synchronizer2.shared.active_chain().tip_header()];
    let sendheaders = SendHeadersBuilder::default()
        .headers(headers.iter().map(|h| h.data()).pack())
        .build();
    assert_eq!(
        HeadersProcess::new(sendheaders.as_reader(), &synchronizer1, peer1, &mock_nc).execute(),
        Status::ok(),
    );

    synchronizer1
        .get_blocks_to_fetch(peer1, IBDState::Out)
        .unwrap();

    let last_common_header2 = synchronizer1.peers().get_last_common_header(peer1).unwrap();
    assert_eq!(
        &last_common_header2.hash(),
        blocks_to_fetch[0].last().unwrap(),
        "last_common_header change because it update during get_blocks_to_fetch",
    );
}

#[cfg(not(disable_faketime))]
#[test]
fn test_header_sync_timeout() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);

    let (_, _, synchronizer) = start_chain(None);

    let network_context = mock_network_context(5);
    faketime::write_millis(&faketime_file, MAX_TIP_AGE * 2).expect("write millis");
    assert!(synchronizer
        .shared
        .active_chain()
        .is_initial_block_download());
    let peers = synchronizer.peers();
    // protect should not effect headers_timeout
    {
        let timeout = HeadersSyncController::new(0, 0, 0, 0, false);
        let not_timeout = HeadersSyncController::new(MAX_TIP_AGE * 2, 0, MAX_TIP_AGE * 2, 0, false);

        let mut state_0 = PeerState::default();
        state_0.peer_flags.is_protect = true;
        state_0.peer_flags.is_outbound = true;
        state_0.headers_sync_controller = Some(timeout);

        let mut state_1 = PeerState::default();
        state_1.peer_flags.is_outbound = true;
        state_1.headers_sync_controller = Some(timeout);

        let mut state_2 = PeerState::default();
        state_2.peer_flags.is_whitelist = true;
        state_2.peer_flags.is_outbound = true;
        state_2.headers_sync_controller = Some(timeout);

        let mut state_3 = PeerState::default();
        state_3.peer_flags.is_outbound = true;
        state_3.headers_sync_controller = Some(not_timeout);

        peers.state.insert(0.into(), state_0);
        peers.state.insert(1.into(), state_1);
        peers.state.insert(2.into(), state_2);
        peers.state.insert(3.into(), state_3);
    }
    synchronizer.eviction(&network_context);
    let disconnected = network_context.disconnected.lock();
    assert_eq!(
        disconnected.deref(),
        &vec![0, 1, 2].into_iter().map(Into::into).collect()
    )
}

#[cfg(not(disable_faketime))]
#[test]
fn test_chain_sync_timeout() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);

    let consensus = Consensus::default();
    let block = BlockBuilder::default()
        .compact_target(difficulty_to_compact(U256::from(3u64)).pack())
        .transaction(consensus.genesis_block().transactions()[0].clone())
        .build();
    let consensus = ConsensusBuilder::default().genesis_block(block).build();

    let (_, shared, synchronizer) = start_chain(Some(consensus));

    assert_eq!(shared.snapshot().total_difficulty(), &U256::from(3u64));

    let network_context = mock_network_context(7);
    let peers = synchronizer.peers();
    //6 peers do not trigger header sync timeout
    let not_timeout = HeadersSyncController::new(MAX_TIP_AGE * 2, 0, MAX_TIP_AGE * 2, 0, false);
    let sync_protected_peer = 0.into();
    {
        let mut state_0 = PeerState::default();
        state_0.peer_flags.is_protect = true;
        state_0.peer_flags.is_outbound = true;
        state_0.headers_sync_controller = Some(not_timeout);

        let mut state_1 = PeerState::default();
        state_1.peer_flags.is_protect = true;
        state_1.peer_flags.is_outbound = true;
        state_1.headers_sync_controller = Some(not_timeout);

        let mut state_2 = PeerState::default();
        state_2.peer_flags.is_protect = true;
        state_2.peer_flags.is_outbound = true;
        state_2.headers_sync_controller = Some(not_timeout);

        let mut state_3 = PeerState::default();
        state_3.peer_flags.is_outbound = true;
        state_3.headers_sync_controller = Some(not_timeout);

        let mut state_4 = PeerState::default();
        state_4.peer_flags.is_outbound = true;
        state_4.headers_sync_controller = Some(not_timeout);

        let mut state_5 = PeerState::default();
        state_5.peer_flags.is_outbound = true;
        state_5.headers_sync_controller = Some(not_timeout);

        let mut state_6 = PeerState::default();
        state_6.peer_flags.is_whitelist = true;
        state_6.peer_flags.is_outbound = true;
        state_6.headers_sync_controller = Some(not_timeout);

        peers.state.insert(0.into(), state_0);
        peers.state.insert(1.into(), state_1);
        peers.state.insert(2.into(), state_2);
        peers.state.insert(3.into(), state_3);
        peers.state.insert(4.into(), state_4);
        peers.state.insert(5.into(), state_5);
        peers.state.insert(6.into(), state_6);
    }
    peers.may_set_best_known_header(0.into(), mock_header_view(1));
    peers.may_set_best_known_header(2.into(), mock_header_view(3));
    peers.may_set_best_known_header(3.into(), mock_header_view(1));
    peers.may_set_best_known_header(5.into(), mock_header_view(3));
    {
        // Protected peer 0 start sync
        peers
            .state
            .get_mut(&sync_protected_peer)
            .unwrap()
            .start_sync(not_timeout);
        synchronizer
            .shared()
            .state()
            .n_sync_started()
            .fetch_add(1, Ordering::AcqRel);
    }
    synchronizer.eviction(&network_context);
    {
        // Protected peer 0 still in sync state
        assert!(peers
            .state
            .get(&sync_protected_peer)
            .unwrap()
            .sync_started(),);
        assert_eq!(
            synchronizer
                .shared()
                .state()
                .n_sync_started()
                .load(Ordering::Acquire),
            1
        );

        assert!({ network_context.disconnected.lock().is_empty() });
        // start sync with protected peer
        //protect peer is protected from disconnection
        assert!(peers
            .state
            .get(&2.into())
            .unwrap()
            .chain_sync
            .work_header
            .is_none());
        // Our best block known by this peer is behind our tip, and we're either noticing
        // that for the first time, OR this peer was able to catch up to some earlier point
        // where we checked against our tip.
        // Either way, set a new timeout based on current tip.
        let (tip, total_difficulty) = {
            let snapshot = shared.snapshot();
            let header = snapshot.tip_header().to_owned();
            let total_difficulty = snapshot.total_difficulty().to_owned();
            (header, total_difficulty)
        };
        assert_eq!(
            peers.state.get(&3.into()).unwrap().chain_sync.work_header,
            Some(tip.clone())
        );
        assert_eq!(
            peers
                .state
                .get(&3.into())
                .unwrap()
                .chain_sync
                .total_difficulty,
            Some(total_difficulty.clone())
        );
        assert_eq!(
            peers.state.get(&4.into()).unwrap().chain_sync.work_header,
            Some(tip)
        );
        assert_eq!(
            peers
                .state
                .get(&4.into())
                .unwrap()
                .chain_sync
                .total_difficulty,
            Some(total_difficulty)
        );
        for proto_id in &[0usize, 1, 3, 4, 6] {
            assert_eq!(
                peers
                    .state
                    .get(&(*proto_id).into())
                    .unwrap()
                    .chain_sync
                    .timeout,
                CHAIN_SYNC_TIMEOUT
            );
        }
    }
    faketime::write_millis(&faketime_file, CHAIN_SYNC_TIMEOUT + 1).expect("write millis");
    synchronizer.eviction(&network_context);
    {
        // No evidence yet that our peer has synced to a chain with work equal to that
        // of our tip, when we first detected it was behind. Send a single getheaders
        // message to give the peer a chance to update us.
        assert!({ network_context.disconnected.lock().is_empty() });
        assert_eq!(
            peers.state.get(&3.into()).unwrap().chain_sync.timeout,
            unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME
        );
        assert_eq!(
            peers.state.get(&4.into()).unwrap().chain_sync.timeout,
            unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME
        );
    }
    faketime::write_millis(
        &faketime_file,
        unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME + 1,
    )
    .expect("write millis");
    synchronizer.eviction(&network_context);
    {
        // Protected peer 0 chain_sync timeout
        assert!(!peers
            .state
            .get(&sync_protected_peer)
            .unwrap()
            .sync_started(),);
        assert_eq!(
            synchronizer
                .shared()
                .state()
                .n_sync_started()
                .load(Ordering::Acquire),
            0
        );

        // Peer(3,4) run out of time to catch up!
        let disconnected = network_context.disconnected.lock();
        assert_eq!(
            disconnected.deref(),
            &vec![3, 4].into_iter().map(Into::into).collect()
        )
    }
}

#[cfg(not(disable_faketime))]
#[test]
fn test_n_sync_started() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);

    let consensus = Consensus::default();
    let block = BlockBuilder::default()
        .compact_target(difficulty_to_compact(U256::from(3u64)).pack())
        .transaction(consensus.genesis_block().transactions()[0].clone())
        .build();
    let consensus = ConsensusBuilder::default().genesis_block(block).build();

    let (_, shared, synchronizer) = start_chain(Some(consensus));

    assert_eq!(shared.snapshot().total_difficulty(), &U256::from(3u64));

    let network_context = mock_network_context(1);
    let peers = synchronizer.peers();
    //6 peers do not trigger header sync timeout
    let not_timeout = HeadersSyncController::new(MAX_TIP_AGE * 2, 0, MAX_TIP_AGE * 2, 0, false);
    let sync_protected_peer = 0.into();

    {
        let mut state_0 = PeerState::default();
        state_0.peer_flags.is_protect = true;
        state_0.peer_flags.is_outbound = true;
        state_0.headers_sync_controller = Some(not_timeout);

        peers.state.insert(0.into(), state_0);
    }

    {
        // Protected peer 0 start sync
        peers
            .state
            .get_mut(&sync_protected_peer)
            .unwrap()
            .start_sync(not_timeout);
        synchronizer
            .shared()
            .state()
            .n_sync_started()
            .fetch_add(1, Ordering::AcqRel);
    }
    synchronizer.eviction(&network_context);

    assert!({ network_context.disconnected.lock().is_empty() });
    faketime::write_millis(&faketime_file, CHAIN_SYNC_TIMEOUT + 1).expect("write millis");
    synchronizer.eviction(&network_context);
    {
        assert!({ network_context.disconnected.lock().is_empty() });
        assert_eq!(
            peers
                .state
                .get(&sync_protected_peer)
                .unwrap()
                .chain_sync
                .timeout,
            unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME
        );
    }

    faketime::write_millis(
        &faketime_file,
        unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME + 1,
    )
    .expect("write millis");
    synchronizer.eviction(&network_context);
    {
        // Protected peer 0 chain_sync timeout
        assert!(!peers
            .state
            .get(&sync_protected_peer)
            .unwrap()
            .sync_started(),);
        assert_eq!(
            synchronizer
                .shared()
                .state()
                .n_sync_started()
                .load(Ordering::Acquire),
            0
        );
    }
    // There may be competition between header sync and eviction, it will case assert panic
    let mut state = peers.state.get_mut(&sync_protected_peer).unwrap();
    synchronizer.shared().state().tip_synced(&mut state);
}

#[test]
// `peer.last_common_header` represents what's the fork point between the local main-chain
// and the peer's mani-chain. It may be unmatched with the current state. So we expect that
// the unmatched last_common_header be fixed during `update_last_common_header`
fn test_fix_last_common_header() {
    //  M1 -> M2 -> M3 -> M4 -> M5 -> M6 (chain M)
    //              \
    //                \-> F4 -> F5 -> F6 -> F7 (chain F)
    let m_ = |number| format!("M{}", number);
    let f_ = |number| format!("F{}", number);
    let mut graph = HashMap::new();
    let mut graph_exts = HashMap::new();

    let main_tip_number = 6u64;
    let fork_tip_number = 7u64;
    let fork_point = 3u64;

    // Construct M chain
    {
        let (chain, shared, _) = start_chain(Some(Consensus::default()));
        for number in 1..=main_tip_number {
            insert_block(&chain, &shared, u128::from(number), number);
        }
        for number in 0..=main_tip_number {
            let block_hash = shared.snapshot().get_block_hash(number).unwrap();
            let block = shared.snapshot().get_block(&block_hash).unwrap();
            let block_ext = shared.snapshot().get_block_ext(&block_hash).unwrap();
            graph.insert(m_(number), block);
            graph_exts.insert(m_(number), block_ext);
        }
    }
    // Construct F chain
    {
        let (chain, shared, _) = start_chain(Some(Consensus::default()));
        for number in 1..=fork_tip_number {
            insert_block(
                &chain,
                &shared,
                u128::from(number % (fork_point + 1)),
                number,
            );
        }
        for number in 0..=fork_tip_number {
            let block_hash = shared.snapshot().get_block_hash(number).unwrap();
            let block = shared.snapshot().get_block(&block_hash).unwrap();
            let block_ext = shared.snapshot().get_block_ext(&block_hash).unwrap();
            graph.insert(f_(number), block);
            graph_exts.insert(f_(number), block_ext);
        }
    }

    // Local has stored M as main-chain, and memoried the headers of F in `SyncState.header_map`
    let (_, _, synchronizer) = start_chain(Some(Consensus::default()));
    for number in 1..=main_tip_number {
        let key = m_(number);
        let block = graph.get(&key).cloned().unwrap();
        synchronizer.chain.process_block(Arc::new(block)).unwrap();
    }
    {
        let nc = mock_network_context(1);
        let peer: PeerIndex = 0.into();
        let fork_headers = (1..=fork_tip_number)
            .map(|number| graph.get(&f_(number)).cloned().unwrap())
            .map(|block| block.header().data())
            .collect::<Vec<_>>();
        let sendheaders = SendHeadersBuilder::default()
            .headers(fork_headers.pack())
            .build();
        synchronizer.on_connected(&nc, peer);
        assert!(
            HeadersProcess::new(sendheaders.as_reader(), &synchronizer, peer, &nc)
                .execute()
                .is_ok()
        );
    }

    // vec![(last_common_header, best_known_header, fixed_last_common_header)]
    let cases = vec![
        (None, "M2", Some("M2")),
        (None, "F5", Some("M3")),
        (None, "M5", Some("M5")),
        (Some("M1"), "M5", Some("M1")),
        (Some("M1"), "F7", Some("M1")),
        (Some("M4"), "F7", Some("M3")),
        (Some("F4"), "M6", Some("M3")),
        (Some("F4"), "F7", Some("F4")),
        (Some("F7"), "M6", Some("M3")), // peer reorganize
    ];

    let nc = mock_network_context(cases.len());
    for (case, (last_common, best_known, fix_last_common)) in cases.into_iter().enumerate() {
        let peer: PeerIndex = case.into();
        synchronizer.on_connected(&nc, peer);

        let last_common_header = last_common.map(|key| graph.get(key).cloned().unwrap().header());
        let best_known_header = {
            let header = graph.get(best_known).cloned().unwrap().header();
            let total_difficulty = graph_exts
                .get(best_known)
                .cloned()
                .unwrap()
                .total_difficulty;
            HeaderView::new(header, total_difficulty)
        };
        if let Some(mut state) = synchronizer.shared.state().peers().state.get_mut(&peer) {
            state.last_common_header = last_common_header;
            state.best_known_header = Some(best_known_header.clone());
        }

        let expected = fix_last_common.map(|mark| mark.to_string());
        let actual = BlockFetcher::new(&synchronizer, peer, IBDState::In)
            .update_last_common_header(&best_known_header)
            .map(|header| {
                if graph
                    .get(&m_(header.number()))
                    .map(|b| b.hash() != header.hash())
                    .unwrap_or(false)
                {
                    f_(header.number())
                } else {
                    m_(header.number())
                }
            });
        assert_eq!(
            expected, actual,
            "Case: {}, last_common: {:?}, best_known: {:?}, expected: {:?}, actual: {:?}",
            case, last_common, best_known, expected, actual,
        );
    }
}

#[test]
fn get_blocks_process() {
    let consensus = Consensus::default();
    let (chain_controller, shared, synchronizer) = start_chain(Some(consensus));

    let num = 2;
    for i in 1..num {
        insert_block(&chain_controller, &shared, u128::from(i), i);
    }

    let genesis_hash = shared.consensus().genesis_hash();
    let message_with_genesis = packed::GetBlocks::new_builder()
        .block_hashes(vec![genesis_hash].pack())
        .build();

    let nc = mock_network_context(1);
    let peer: PeerIndex = 1.into();
    let process = GetBlocksProcess::new(message_with_genesis.as_reader(), &synchronizer, peer, &nc);
    assert_eq!(
        process.execute(),
        StatusCode::RequestGenesis.with_context("Request genesis block")
    );

    let hash = shared.snapshot().get_block_hash(1).unwrap();
    let message_with_dup = packed::GetBlocks::new_builder()
        .block_hashes(vec![hash.clone(), hash].pack())
        .build();

    let nc = mock_network_context(1);
    let peer: PeerIndex = 1.into();
    let process = GetBlocksProcess::new(message_with_dup.as_reader(), &synchronizer, peer, &nc);
    assert_eq!(
        process.execute(),
        StatusCode::RequestDuplicate.with_context("Request duplicate block")
    );
}

#[test]
fn test_internal_db_error() {
    use crate::utils::is_internal_db_error;

    let consensus = Consensus::default();
    let mut builder = SharedBuilder::with_temp_db();
    builder = builder.consensus(consensus);

    let (shared, mut pack) = builder.build().unwrap();

    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let _chain_controller = chain_service.start::<&str>(None);

    let sync_shared = Arc::new(SyncShared::new(
        shared,
        Default::default(),
        pack.take_relay_tx_receiver(),
    ));

    let mut chain_controller = ChainController::faux();
    let block = Arc::new(BlockBuilder::default().build());

    // mock process_block
    faux::when!(chain_controller.process_block(Arc::clone(&block))).then_return(Err(
        InternalErrorKind::Database.other("mocked db error").into(),
    ));

    faux::when!(chain_controller.try_stop()).then_return(());

    let synchronizer = Synchronizer::new(chain_controller, sync_shared);

    let status = synchronizer
        .shared()
        .accept_block(&synchronizer.chain, Arc::clone(&block));

    assert!(is_internal_db_error(&status.err().unwrap()));
}
