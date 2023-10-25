use crate::chain::{ChainController, ChainService};
use ckb_app_config::TxPoolConfig;
use ckb_app_config::{BlockAssemblerConfig, NetworkConfig};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_network::{Flags, NetworkController, NetworkService, NetworkState};
use ckb_shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_test_chain_utils::{always_success_cell, create_always_success_tx};
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, Capacity, EpochNumberWithFraction, HeaderView,
        TransactionBuilder, TransactionView,
    },
    h256,
    packed::{CellInput, CellOutput, OutPoint},
    utilities::DIFF_TWO,
};
use std::sync::Arc;

pub(crate) fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared, HeaderView) {
    start_chain_with_tx_pool_config(consensus, TxPoolConfig::default())
}

pub(crate) fn start_chain_with_tx_pool_config(
    consensus: Option<Consensus>,
    tx_pool_config: TxPoolConfig,
) -> (ChainController, Shared, HeaderView) {
    let builder = SharedBuilder::with_temp_db();
    let (_, _, always_success_script) = always_success_cell();
    let consensus = consensus.unwrap_or_else(|| {
        let tx = create_always_success_tx();
        let dao = genesis_dao_data(vec![&tx]).unwrap();
        // create genesis block with N txs
        let transactions: Vec<TransactionView> = (0..10u64)
            .map(|i| {
                let data = Bytes::from(i.to_le_bytes().to_vec());
                TransactionBuilder::default()
                    .input(CellInput::new(OutPoint::null(), 0))
                    .output(
                        CellOutput::new_builder()
                            .capacity(capacity_bytes!(50_000).pack())
                            .lock(always_success_script.clone())
                            .build(),
                    )
                    .output_data(data.pack())
                    .build()
            })
            .collect();

        let genesis_block = BlockBuilder::default()
            .dao(dao)
            .compact_target(DIFF_TWO.pack())
            .transaction(tx)
            .transactions(transactions)
            .build();
        ConsensusBuilder::default()
            .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
            .genesis_block(genesis_block)
            .build()
    });

    let config = BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: Default::default(),
        hash_type: ScriptHashType::Data,
        message: Default::default(),
        use_binary_version_as_message_prefix: false,
        binary_version: "TEST".to_string(),
        update_interval_millis: 800,
        notify: vec![],
        notify_scripts: vec![],
        notify_timeout_millis: 800,
    };

    let (shared, mut pack) = builder
        .consensus(consensus)
        .tx_pool_config(tx_pool_config)
        .block_assembler_config(Some(config))
        .build()
        .unwrap();
    let network = dummy_network(&shared);
    pack.take_tx_pool_builder().start(network);

    let _chain_service = ChainService::new(
        shared.clone(),
        pack.take_proposal_table(),
        pack.take_verify_failed_block_tx(),
    );
    let chain_controller = _chain_service.start::<&str>(Some("ckb_chain::tests::ChainService"));
    let parent = {
        let snapshot = shared.snapshot();
        snapshot
            .get_block_hash(0)
            .and_then(|hash| snapshot.get_block_header(&hash))
            .unwrap()
    };

    (chain_controller, shared, parent)
}

pub(crate) fn dummy_network(shared: &Shared) -> NetworkController {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let config = NetworkConfig {
        max_peers: 19,
        max_outbound_peers: 5,
        path: tmp_dir.path().to_path_buf(),
        ping_interval_secs: 15,
        ping_timeout_secs: 20,
        connect_outbound_interval_secs: 1,
        discovery_local_address: true,
        bootnode_mode: true,
        reuse_port_on_linux: true,
        ..Default::default()
    };

    let network_state =
        Arc::new(NetworkState::from_config(config).expect("Init network state failed"));
    NetworkService::new(
        network_state,
        vec![],
        vec![],
        (
            shared.consensus().identify_name(),
            "test".to_string(),
            Flags::COMPATIBILITY,
        ),
    )
    .start(shared.async_handle())
    .expect("Start network service failed")
}
