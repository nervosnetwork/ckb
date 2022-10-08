use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use ckb_app_config::{BlockAssemblerConfig, NetworkConfig};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{build_genesis_epoch_ext, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_launcher::SharedBuilder;
use ckb_network::{DefaultExitHandler, Flags, NetworkController, NetworkService, NetworkState};
use ckb_shared::Shared;
use ckb_sync::SyncShared;
use ckb_test_chain_utils::always_success_cell;
use ckb_tx_pool::TxPoolController;
use ckb_types::{
    core::{BlockNumber, BlockView, Capacity, EpochNumberWithFraction, TransactionView},
    packed,
    prelude::*,
    utilities::DIFF_TWO,
};
use faketime::unix_time_as_millis;

use crate::{tests::prelude::*, LightClientProtocol};

pub(crate) struct MockChain {
    chain_controller: ChainController,
    sync_shared: Arc<SyncShared>,
    always_success_cell_dep: packed::CellDep,
}

impl MockChain {
    pub(crate) fn new() -> Self {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();

        let always_success_tx = TransactionView::new_advanced_builder()
            .input(packed::CellInput::new(packed::OutPoint::null(), 0))
            .output(always_success_cell.clone())
            .output_data(always_success_cell_data.pack())
            .witness(always_success_script.clone().into_witness())
            .build();
        let always_success_out_point = packed::OutPoint::new(always_success_tx.hash(), 0);
        let always_success_cell_dep = packed::CellDep::new_builder()
            .out_point(always_success_out_point)
            .build();

        let dao = genesis_dao_data(vec![&always_success_tx]).unwrap();

        let (shared, mut pack) = {
            let genesis = BlockView::new_advanced_builder()
                .timestamp(unix_time_as_millis().pack())
                .compact_target(DIFF_TWO.pack())
                .transaction(always_success_tx)
                .dao(dao)
                .build();
            let epoch_ext = build_genesis_epoch_ext(
                Capacity::shannons(100_000_000_000_000),
                DIFF_TWO,
                10,
                4 * 60 * 60,
                (1, 40),
            );
            let consensus = ConsensusBuilder::new(genesis, epoch_ext)
                .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
                .permanent_difficulty_in_dummy(true)
                .build();
            let config = BlockAssemblerConfig {
                // always success
                code_hash: always_success_script.code_hash().unpack(),
                args: Default::default(),
                hash_type: ScriptHashType::Data,
                message: Default::default(),
                use_binary_version_as_message_prefix: true,
                binary_version: "LightClientServer".to_string(),
                update_interval_millis: 800,
                notify: vec![],
                notify_scripts: vec![],
                notify_timeout_millis: 800,
            };
            SharedBuilder::with_temp_db()
                .consensus(consensus)
                .block_assembler_config(Some(config))
                .build()
                .unwrap()
        };

        let network = dummy_network(&shared);
        pack.take_tx_pool_builder().start(network);

        let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
        let chain_controller = chain_service.start::<&str>(None);

        let sync_shared = Arc::new(SyncShared::new(
            shared,
            Default::default(),
            pack.take_relay_tx_receiver(),
        ));

        Self {
            chain_controller,
            sync_shared,
            always_success_cell_dep,
        }
    }

    pub(crate) fn controller(&self) -> &ChainController {
        &self.chain_controller
    }

    pub(crate) fn shared(&self) -> &Shared {
        self.sync_shared.shared()
    }

    pub(crate) fn tx_pool(&self) -> &TxPoolController {
        self.shared().tx_pool_controller()
    }

    pub(crate) fn always_success_cell_dep(&self) -> packed::CellDep {
        self.always_success_cell_dep.clone()
    }

    pub(crate) fn create_light_client_protocol(&self) -> LightClientProtocol {
        let shared = Arc::clone(&self.sync_shared);
        LightClientProtocol::new(shared)
    }

    pub(crate) fn mine_to(&self, block_number: BlockNumber) {
        let chain_tip_number = self.shared().snapshot().tip_number();
        if chain_tip_number < block_number {
            self.mine_blocks((block_number - chain_tip_number) as usize);
        }
    }

    pub(crate) fn mine_blocks(&self, blocks_count: usize) {
        for _ in 0..blocks_count {
            let _ = self.mine_block(|block| block.as_advanced_builder().build());
        }
    }

    pub(crate) fn mine_block<F: FnMut(packed::Block) -> BlockView>(
        &self,
        mut build: F,
    ) -> BlockNumber {
        let block_template = self
            .shared()
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap();
        let block: packed::Block = block_template.into();
        let block = build(block);
        let block_number = block.number();
        let is_ok = self
            .controller()
            .process_block(Arc::new(block))
            .expect("process block");
        assert!(is_ok, "failed to process block {}", block_number);
        while self
            .tx_pool()
            .get_tx_pool_info()
            .expect("get tx pool info")
            .tip_number
            != block_number
        {}
        block_number
    }

    pub(crate) fn rollback_to(
        &self,
        target_number: BlockNumber,
        detached_proposal_ids: HashSet<packed::ProposalShortId>,
    ) {
        let snapshot = self.shared().snapshot();

        let chain_tip_number = snapshot.tip_number();

        let mut detached_blocks = VecDeque::default();
        for num in (target_number + 1)..=chain_tip_number {
            let detached_block = snapshot.get_block_by_number(num).unwrap();
            detached_blocks.push_back(detached_block);
        }

        let target_hash = snapshot.get_header_by_number(target_number).unwrap().hash();
        self.controller().truncate(target_hash).unwrap();
        self.tx_pool()
            .update_tx_pool_for_reorg(
                detached_blocks,
                VecDeque::default(),
                detached_proposal_ids,
                Arc::clone(&self.shared().snapshot()),
            )
            .unwrap();

        while self.shared().snapshot().tip_number() != target_number {}
        while self
            .tx_pool()
            .get_tx_pool_info()
            .expect("get tx pool info")
            .tip_number
            != self.shared().snapshot().tip_number()
        {}
    }

    pub(crate) fn get_cellbase_as_input(&self, block_number: BlockNumber) -> TransactionView {
        let snapshot = self.shared().snapshot();
        let block = snapshot.get_block_by_number(block_number).unwrap();
        let cellbase = block.transaction(0).unwrap();
        let input = packed::CellInput::new(packed::OutPoint::new(cellbase.hash(), 0), 0);
        let input_capacity: Capacity = cellbase.output(0).unwrap().capacity().unpack();
        let output_capacity = input_capacity.safe_sub(1000u32).unwrap();
        let header_dep = block.hash();
        let (_, _, always_success_script) = always_success_cell();
        let output = packed::CellOutput::new_builder()
            .capacity(output_capacity.pack())
            .lock(always_success_script.to_owned())
            .build();
        TransactionView::new_advanced_builder()
            .cell_dep(self.always_success_cell_dep())
            .header_dep(header_dep)
            .input(input)
            .output(output)
            .output_data(Default::default())
            .build()
    }
}

fn dummy_network(shared: &Shared) -> NetworkController {
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
            Flags::all(),
        ),
        DefaultExitHandler::default(),
    )
    .start(shared.async_handle())
    .expect("Start network service failed")
}
