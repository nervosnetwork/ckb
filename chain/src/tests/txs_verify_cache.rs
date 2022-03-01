use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::genesis_dao_data;
use ckb_launcher::SharedBuilder;
use ckb_shared::Shared;
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    bytes,
    core::{
        capacity_bytes, hardfork::HardForkSwitch, BlockNumber, BlockView, Capacity, DepType,
        EpochExt, EpochNumberWithFraction, HeaderView, ScriptHashType, TransactionView,
    },
    packed,
    prelude::*,
    utilities::difficulty_to_compact,
    U256,
};
use ckb_verification_traits::Switch;
use faketime::unix_time_as_millis;
use lazy_static::lazy_static;

use std::{fs::File, io::Read as _, path::Path, sync::Arc};

use crate::{
    chain::{ChainController, ChainService},
    tests::util::dummy_network,
};

const CYCLES_IN_VM0: u64 = 696;
const CYCLES_IN_VM1: u64 = 686;

lazy_static! {
    static ref LOCK_SCRIPT_CELL: (packed::CellOutput, bytes::Bytes, packed::Script) =
        create_lock_script_cell();
}

fn create_lock_script_cell() -> (packed::CellOutput, bytes::Bytes, packed::Script) {
    let mut buffer = Vec::new();
    {
        let file_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/mop_adc_lock");
        let mut file = File::open(file_path).unwrap();
        file.read_to_end(&mut buffer).unwrap();
    }

    let data: bytes::Bytes = buffer.into();

    let (_, _, always_success_script) = always_success_cell();
    let cell = packed::CellOutput::new_builder()
        .type_(Some(always_success_script.clone()).pack())
        .capacity(Capacity::bytes(data.len()).unwrap().pack())
        .build();

    let script = packed::Script::new_builder()
        .hash_type(ScriptHashType::Data.into())
        .code_hash(packed::CellOutput::calc_data_hash(&data))
        .build();

    (cell, data, script)
}

fn lock_script_cell() -> &'static (packed::CellOutput, bytes::Bytes, packed::Script) {
    &LOCK_SCRIPT_CELL
}

// Creates a consensus with:
// - Fixed epoch length.
// - Hardfork for feature RFC-0032.
// - A cell of a lock script which has different cycles in different VMs.
fn create_consensus() -> (
    Consensus,
    Vec<packed::CellDep>,
    packed::Script,
    TransactionView,
) {
    let epoch_length = 10;
    let epoch_when_enable_vm1 = 2;
    let (always_success_cell, always_success_data, always_success_script) = always_success_cell();
    let (lock_script_cell, lock_script_data, lock_script) = lock_script_cell();

    let deploy_always_success_tx = TransactionView::new_advanced_builder()
        .input(packed::CellInput::new_cellbase_input(0))
        .output(always_success_cell.clone())
        .output_data(always_success_data.pack())
        .witness(always_success_script.clone().into_witness())
        .build();
    let always_success_cell_dep = {
        let always_success_cell_op = packed::OutPoint::new(deploy_always_success_tx.hash(), 0);
        packed::CellDep::new_builder()
            .out_point(always_success_cell_op)
            .dep_type(DepType::Code.into())
            .build()
    };
    let deploy_lock_script_tx = TransactionView::new_advanced_builder()
        .cell_dep(always_success_cell_dep)
        .input(packed::CellInput::new_cellbase_input(0))
        .output(lock_script_cell.clone())
        .output_data(lock_script_data.pack())
        .witness(lock_script.clone().into_witness())
        .build();
    let lock_script_cell_dep = {
        let lock_script_cell_op = packed::OutPoint::new(deploy_lock_script_tx.hash(), 0);
        packed::CellDep::new_builder()
            .out_point(lock_script_cell_op)
            .dep_type(DepType::Code.into())
            .build()
    };
    let lock_script_via_type = {
        let type_hash = always_success_script.calc_script_hash();
        packed::Script::new_builder()
            .code_hash(type_hash)
            .hash_type(ScriptHashType::Type.into())
            .build()
    };
    let input_tx = TransactionView::new_advanced_builder()
        .cell_dep(lock_script_cell_dep.clone())
        .input(packed::CellInput::new_cellbase_input(0))
        .output(
            packed::CellOutput::new_builder()
                .capacity(capacity_bytes!(1_000_000).pack())
                .lock(lock_script_via_type.clone())
                .build(),
        )
        .output_data(Default::default())
        .witness(Default::default())
        .build();

    let dao = genesis_dao_data(vec![&deploy_always_success_tx, &deploy_lock_script_tx]).unwrap();
    let genesis = packed::Block::new_advanced_builder()
        .timestamp(unix_time_as_millis().pack())
        .compact_target(difficulty_to_compact(U256::from(100u64)).pack())
        .dao(dao)
        .transaction(deploy_always_success_tx)
        .transaction(deploy_lock_script_tx)
        .transaction(input_tx.clone())
        .build();

    let hardfork_switch = HardForkSwitch::new_without_any_enabled()
        .as_builder()
        .rfc_0032(epoch_when_enable_vm1)
        .build()
        .unwrap();
    let mut consensus = Consensus {
        permanent_difficulty_in_dummy: true,
        hardfork_switch,
        genesis_block: genesis,
        cellbase_maturity: EpochNumberWithFraction::new(0, 0, 1),
        ..Default::default()
    };
    consensus.genesis_epoch_ext.set_length(epoch_length);

    (
        consensus,
        vec![lock_script_cell_dep],
        lock_script_via_type,
        input_tx,
    )
}

fn start_chain(consensus: Consensus, lock_script: &packed::Script) -> (ChainController, Shared) {
    let hash_type: ScriptHashType = lock_script.hash_type().try_into().unwrap();
    let config = BlockAssemblerConfig {
        code_hash: lock_script.code_hash().unpack(),
        args: Default::default(),
        hash_type: hash_type.into(),
        message: Default::default(),
        use_binary_version_as_message_prefix: true,
        binary_version: "TEST".to_string(),
    };
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(consensus)
        .block_assembler_config(Some(config))
        .build()
        .unwrap();

    let network = dummy_network(&shared);
    pack.take_tx_pool_builder().start(network);

    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn create_cellbase(
    number: BlockNumber,
    epoch: &EpochExt,
    lock_script: &packed::Script,
) -> TransactionView {
    TransactionView::new_advanced_builder()
        .input(packed::CellInput::new_cellbase_input(number))
        .output(
            packed::CellOutput::new_builder()
                .capacity(epoch.block_reward(number).unwrap().pack())
                .lock(lock_script.clone())
                .build(),
        )
        .output_data(Default::default())
        .witness(packed::Script::default().into_witness())
        .build()
}

fn generate_block(
    parent_header: &HeaderView,
    nonce: u128,
    epoch: &EpochExt,
    lock_script: &packed::Script,
) -> BlockView {
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number, epoch, lock_script);
    let dao = genesis_dao_data(vec![&cellbase]).unwrap();
    let header = HeaderView::new_advanced_builder()
        .parent_hash(parent_header.hash())
        .timestamp((parent_header.timestamp() + 10).pack())
        .number(number.pack())
        .epoch(epoch.number_with_fraction(number).pack())
        .compact_target(epoch.compact_target().pack())
        .nonce(nonce.pack())
        .dao(dao)
        .build();
    packed::Block::new_advanced_builder()
        .header(header)
        .transaction(cellbase)
        .build_unchecked()
}

fn build_tx(
    parent_tx: &TransactionView,
    cell_deps: &[packed::CellDep],
    lock_script: &packed::Script,
) -> TransactionView {
    let input_op = packed::OutPoint::new(parent_tx.hash(), 0);
    let input = packed::CellInput::new(input_op, 0);
    let output_capacity = Capacity::shannons(parent_tx.output(0).unwrap().capacity().unpack())
        .safe_sub(Capacity::bytes(1).unwrap())
        .unwrap();
    let output = packed::CellOutput::new_builder()
        .capacity(output_capacity.pack())
        .lock(lock_script.clone())
        .build();
    let mut tx_builder = TransactionView::new_advanced_builder();
    for cell_dep in cell_deps {
        tx_builder = tx_builder.cell_dep(cell_dep.clone());
    }
    tx_builder
        .input(input)
        .output(output)
        .output_data(Default::default())
        .build()
}

#[test]
fn refresh_txs_verify_cache_after_hardfork() {
    let (consensus, cell_deps, lock_script, input_tx) = create_consensus();
    let epoch_when_enable_vm1 = consensus.hardfork_switch.vm_version_1_and_syscalls_2();

    let (chain_controller, shared) = start_chain(consensus, &lock_script);

    // set to genesis header
    let mut parent_header = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let tx_pool = shared.tx_pool_controller();

    let tx = build_tx(&input_tx, &cell_deps, &lock_script);
    let tx_size = tx.data().serialized_size_in_block();
    tx_pool.submit_local_tx(tx.clone()).unwrap().unwrap();

    // at start of the test, the script should be ran with vm0
    {
        let tx_pool_entries = tx_pool.get_all_entry_info().unwrap();
        let mut counter = 0;
        loop {
            if let Some(tx_entry) = tx_pool_entries.pending.get(&tx.hash()) {
                assert_eq!(tx_entry.cycles, CYCLES_IN_VM0);
                let tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
                assert_eq!(tx_pool_info.total_tx_size, tx_size);
                assert_eq!(tx_pool_info.total_tx_cycles, CYCLES_IN_VM0);
                break;
            }
            // wait tx_pool if got `None`
            counter += 1;
            if counter > 100 {
                panic!("tx-pool is too slow to refresh caches");
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }

    for _ in 0..40 {
        let epoch = shared
            .consensus()
            .next_epoch_ext(&parent_header, &shared.store().as_data_provider())
            .unwrap()
            .epoch();
        let block = generate_block(&parent_header, 0, &epoch, &lock_script);

        let cycles_expected = if epoch.number() >= epoch_when_enable_vm1 {
            CYCLES_IN_VM1
        } else {
            CYCLES_IN_VM0
        };

        let is_in_delay_window = shared
            .consensus()
            .is_in_delay_window(&parent_header.epoch());
        let mut counter = 0;
        loop {
            let tx_pool_entries = tx_pool.get_all_entry_info().unwrap();
            if is_in_delay_window {
                break;
            }

            if let Some(tx_entry) = tx_pool_entries.pending.get(&tx.hash()) {
                assert_eq!(
                    tx_entry.cycles,
                    cycles_expected,
                    "block = {}, epoch = {}, cycles should be {}, but got {}",
                    block.number(),
                    epoch.number(),
                    cycles_expected,
                    tx_entry.cycles,
                );
                let tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
                assert_eq!(tx_pool_info.total_tx_size, tx_size);
                assert_eq!(tx_pool_info.total_tx_cycles, cycles_expected);
                break;
            }
            // wait tx_pool if got `None`
            counter += 1;
            if counter > 100 {
                panic!("tx-pool is too slow to refresh caches");
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }

        chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::ONLY_SCRIPT)
            .expect("process block");
        parent_header = block.header().to_owned();
    }

    // at last of the test, the script should be ran with vm1
    {
        let tx_pool_entries = tx_pool.get_all_entry_info().unwrap();
        let tx_entry = tx_pool_entries.pending.get(&tx.hash()).unwrap();
        assert_eq!(tx_entry.cycles, CYCLES_IN_VM1);
        let tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
        assert_eq!(tx_pool_info.total_tx_size, tx_size);
        assert_eq!(tx_pool_info.total_tx_cycles, CYCLES_IN_VM1);
    }
}
