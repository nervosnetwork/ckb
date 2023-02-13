use crate::chain::{ChainController, ChainService};
use crate::tests::util::dummy_network;
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::genesis_dao_data;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_launcher::SharedBuilder;
use ckb_shared::Shared;
use ckb_shared::Snapshot;
use ckb_store::ChainStore;
use ckb_tx_pool::{block_assembler::CandidateUncles, PlugTarget, TxEntry};
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockBuilder, BlockNumber, BlockView, Capacity, EpochExt, HeaderBuilder, HeaderView,
        TransactionBuilder, TransactionView,
    },
    h256,
    packed::{Block, CellInput, CellOutput, CellOutputBuilder, CellbaseWitness, OutPoint},
    prelude::*,
};
use ckb_verification::{BlockVerifier, HeaderVerifier};
use ckb_verification_traits::{Switch, Verifier};
use lazy_static::lazy_static;
use std::sync::Arc;

fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared) {
    let mut builder = SharedBuilder::with_temp_db();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let config = BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: Default::default(),
        hash_type: ScriptHashType::Data,
        message: Default::default(),
        use_binary_version_as_message_prefix: true,
        binary_version: "TEST".to_string(),
        update_interval_millis: 800,
        notify: vec![],
        notify_scripts: vec![],
        notify_timeout_millis: 800,
    };
    let (shared, mut pack) = builder
        .block_assembler_config(Some(config))
        .build()
        .unwrap();

    let network = dummy_network(&shared);
    pack.take_tx_pool_builder().start(network);

    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

lazy_static! {
    static ref BASIC_BLOCK_SIZE: u64 = {
        let (_chain_controller, shared) = start_chain(None);

        let block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap();

        let block: Block = block_template.into();
        block.serialized_size_without_uncle_proposals() as u64
    };
}

#[test]
fn test_get_block_template() {
    let (_chain_controller, shared) = start_chain(None);

    let block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();
    let header = block.header();

    let header_verify_result = {
        let snapshot: &Snapshot = &shared.snapshot();
        let header_verifier = HeaderVerifier::new(snapshot, shared.consensus());
        header_verifier.verify(&header)
    };
    assert!(header_verify_result.is_ok());

    let block_verify = BlockVerifier::new(shared.consensus());
    assert!(block_verify.verify(&block).is_ok());
}

fn gen_block(parent_header: &HeaderView, nonce: u128, epoch: &EpochExt) -> BlockView {
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number, epoch);
    // This just make sure we can generate a valid block template,
    // the actual DAO validation logic will be ensured in other
    // tests
    let dao = genesis_dao_data(vec![&cellbase]).unwrap();
    let header = HeaderBuilder::default()
        .parent_hash(parent_header.hash())
        .timestamp((parent_header.timestamp() + 10).pack())
        .number(number.pack())
        .epoch(epoch.number_with_fraction(number).pack())
        .compact_target(epoch.compact_target().pack())
        .nonce(nonce.pack())
        .dao(dao)
        .build();

    BlockBuilder::default()
        .header(header)
        .transaction(cellbase)
        .proposal([1; 10].pack())
        .build_unchecked()
}

fn create_cellbase(number: BlockNumber, epoch: &EpochExt) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(
            CellOutput::new_builder()
                .capacity(epoch.block_reward(number).unwrap().pack())
                .build(),
        )
        .output_data(Default::default())
        .build()
}

#[cfg(not(disable_faketime))]
#[test]
fn test_block_template_timestamp() {
    let mut _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);

    let consensus = Consensus::default();
    let epoch = consensus.genesis_epoch_ext().clone();
    let (chain_controller, shared) = start_chain(Some(consensus));

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let block = gen_block(&genesis, 0, &epoch);

    chain_controller
        .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
        .unwrap();

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    while (Into::<u64>::into(block_template.number)) != 2 {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }
    assert_eq!(
        Into::<u64>::into(block_template.current_time),
        block.header().timestamp() + 1
    );
}

#[test]
fn test_block_template_message() {
    let (_chain_controller, shared) = start_chain(None);

    let block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    let cellbase_witness = CellbaseWitness::from_slice(
        block_template
            .cellbase
            .data
            .witnesses
            .get(0)
            .unwrap()
            .as_bytes(),
    )
    .expect("should be valid CellbaseWitness slice");
    let snapshot = shared.snapshot();
    let version = snapshot
        .compute_versionbits(snapshot.tip_header())
        .unwrap()
        .to_le_bytes();
    assert_eq!(
        [version.as_slice(), b" ", "TEST".as_bytes()].concat(),
        cellbase_witness.message().raw_data()
    );
}

#[test]
fn test_prepare_uncles() {
    let mut consensus = Consensus::default();
    consensus.genesis_epoch_ext.set_length(5);
    let epoch = consensus.genesis_epoch_ext().clone();

    let (chain_controller, shared) = start_chain(Some(consensus));

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let block0_0 = gen_block(&genesis, 11, &epoch);
    let block0_1 = gen_block(&genesis, 10, &epoch);
    let block1_1 = gen_block(&block0_1.header(), 10, &epoch);

    chain_controller
        .internal_process_block(Arc::new(block0_1), Switch::DISABLE_ALL)
        .unwrap();
    chain_controller
        .internal_process_block(Arc::new(block0_0.clone()), Switch::DISABLE_ALL)
        .unwrap();
    chain_controller
        .internal_process_block(Arc::new(block1_1.clone()), Switch::DISABLE_ALL)
        .unwrap();

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    // block number 3, epoch 0
    while (Into::<u64>::into(block_template.number)) != 3 || block_template.uncles.is_empty() {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }
    assert_eq!(block_template.uncles[0].hash, block0_0.hash().unpack());

    let epoch = shared
        .consensus()
        .next_epoch_ext(&block1_1.header(), &shared.store().as_data_provider())
        .unwrap()
        .epoch();

    let block2_1 = gen_block(&block1_1.header(), 10, &epoch);
    chain_controller
        .internal_process_block(Arc::new(block2_1.clone()), Switch::DISABLE_ALL)
        .unwrap();

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    while Into::<u64>::into(block_template.number) != 4 || block_template.uncles.is_empty() {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }
    // block number 4, epoch 0, uncles should retained
    assert_eq!(block_template.uncles[0].hash, block0_0.hash().unpack());

    let epoch = shared
        .consensus()
        .next_epoch_ext(&block2_1.header(), &shared.store().as_data_provider())
        .unwrap()
        .epoch();

    let block3_1 = gen_block(&block2_1.header(), 10, &epoch);
    chain_controller
        .internal_process_block(Arc::new(block3_1), Switch::DISABLE_ALL)
        .unwrap();

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    while (Into::<u64>::into(block_template.number)) != 5 {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }
    // block number 5, epoch 1, block_template should not include last epoch uncles
    assert!(block_template.uncles.is_empty());
}

#[test]
fn test_candidate_uncles_retain() {
    let mut consensus = Consensus::default();
    consensus.genesis_epoch_ext.set_length(5);
    let epoch = consensus.genesis_epoch_ext().clone();
    let mut candidate_uncles = CandidateUncles::new();

    let (chain_controller, shared) = start_chain(Some(consensus));

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let block0_0 = gen_block(&genesis, 11, &epoch);
    let block0_1 = gen_block(&genesis, 10, &epoch);
    let block1_1 = gen_block(&block0_1.header(), 10, &epoch);

    chain_controller
        .internal_process_block(Arc::new(block0_1), Switch::DISABLE_ALL)
        .unwrap();
    chain_controller
        .internal_process_block(Arc::new(block0_0.clone()), Switch::DISABLE_ALL)
        .unwrap();
    chain_controller
        .internal_process_block(Arc::new(block1_1.clone()), Switch::DISABLE_ALL)
        .unwrap();

    candidate_uncles.insert(block0_0.as_uncle());

    {
        let snapshot = shared.snapshot();
        let epoch = shared
            .consensus()
            .next_epoch_ext(&block1_1.header(), &shared.store().as_data_provider())
            .unwrap()
            .epoch();
        let uncles = candidate_uncles.prepare_uncles(&snapshot, &epoch);

        assert_eq!(uncles[0].hash(), block0_0.hash());
    }

    let block1_0 = gen_block(&block0_0.header(), 12, &epoch);
    let block2_0 = gen_block(&block1_0.header(), 13, &epoch);
    for block in vec![block1_0, block2_0.clone()] {
        chain_controller
            .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
            .unwrap();
    }

    {
        let snapshot = shared.snapshot();
        let uncles = candidate_uncles.prepare_uncles(&snapshot, &epoch);
        assert!(uncles.is_empty());
        // candidate uncles should retain
        assert!(candidate_uncles.contains(&block0_0.as_uncle()));
    }

    let epoch = shared
        .consensus()
        .next_epoch_ext(&block2_0.header(), &shared.store().as_data_provider())
        .unwrap()
        .epoch();

    let block3_0 = gen_block(&block2_0.header(), 10, &epoch);
    chain_controller
        .internal_process_block(Arc::new(block3_0.clone()), Switch::DISABLE_ALL)
        .unwrap();

    {
        let snapshot = shared.snapshot();
        let epoch = shared
            .consensus()
            .next_epoch_ext(&block3_0.header(), &shared.store().as_data_provider())
            .unwrap()
            .epoch();
        let uncles = candidate_uncles.prepare_uncles(&snapshot, &epoch);
        assert!(uncles.is_empty());
        // candidate uncles should remove by next epoch
        assert!(candidate_uncles.is_empty());
    }
}

fn build_tx(parent_tx: &TransactionView, inputs: &[u32], outputs_len: usize) -> TransactionView {
    let per_output_capacity =
        Capacity::shannons(parent_tx.outputs_capacity().unwrap().as_u64() / outputs_len as u64);
    TransactionBuilder::default()
        .inputs(
            inputs
                .iter()
                .map(|index| CellInput::new(OutPoint::new(parent_tx.hash(), *index), 0)),
        )
        .outputs(
            (0..outputs_len)
                .map(|_| {
                    CellOutputBuilder::default()
                        .capacity(per_output_capacity.pack())
                        .build()
                })
                .collect::<Vec<CellOutput>>(),
        )
        .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
        .build()
}

fn check_txs(entities: &[TxEntry], expect_txs: Vec<&TransactionView>, format_arg: &str) {
    assert_eq!(
        entities
            .iter()
            .map(|entry| entry.transaction().hash())
            .collect::<Vec<_>>(),
        expect_txs.iter().map(|tx| tx.hash()).collect::<Vec<_>>(),
        "{format_arg}"
    );
}

#[test]
fn test_package_basic() {
    let mut consensus = Consensus::default();
    consensus.genesis_epoch_ext.set_length(5);
    let epoch = consensus.genesis_epoch_ext().clone();

    let (chain_controller, shared) = start_chain(Some(consensus));

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mut parent_header = genesis;
    let mut blocks = vec![];
    for _i in 0..4 {
        let block = gen_block(&parent_header, 11, &epoch);
        chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block");
        parent_header = block.header().to_owned();
        blocks.push(block);
    }

    let tx0 = &blocks[0].transactions()[0];
    let tx1 = build_tx(tx0, &[0], 2);
    let tx2 = build_tx(&tx1, &[0], 2);
    let tx3 = build_tx(&tx2, &[0], 2);
    let tx4 = build_tx(&tx3, &[0], 2);

    let tx2_0 = &blocks[1].transactions()[0];
    let tx2_1 = build_tx(tx2_0, &[0], 2);
    let tx2_2 = build_tx(&tx2_1, &[0], 2);
    let tx2_3 = build_tx(&tx2_2, &[0], 2);

    let tx_pool = shared.tx_pool_controller();
    let entries = vec![
        TxEntry::dummy_resolve(tx1.clone(), 0, Capacity::shannons(100), 100),
        TxEntry::dummy_resolve(tx2.clone(), 0, Capacity::shannons(100), 100),
        TxEntry::dummy_resolve(tx3.clone(), 0, Capacity::shannons(100), 100),
        TxEntry::dummy_resolve(tx4.clone(), 0, Capacity::shannons(1500), 500),
        TxEntry::dummy_resolve(tx2_1.clone(), 0, Capacity::shannons(150), 100),
        TxEntry::dummy_resolve(tx2_2.clone(), 0, Capacity::shannons(150), 100),
        TxEntry::dummy_resolve(tx2_3.clone(), 0, Capacity::shannons(150), 100),
    ];
    tx_pool.plug_entry(entries, PlugTarget::Proposed).unwrap();

    // 300 size best scored txs
    let txs = tx_pool.package_txs(Some(300)).unwrap();

    check_txs(
        &txs,
        vec![&tx2_1, &tx2_2, &tx2_3],
        "300 size best scored txs",
    );

    // 400 size best scored txs
    let txs = tx_pool.package_txs(Some(400)).unwrap();
    check_txs(
        &txs,
        vec![&tx2_1, &tx2_2, &tx2_3, &tx1],
        "400 size best scored txs",
    );

    // 500 size best scored txs
    let txs = tx_pool.package_txs(Some(500)).unwrap();
    check_txs(
        &txs,
        vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2],
        "500 size best scored txs",
    );

    // 600 size best scored txs
    let txs = tx_pool.package_txs(Some(600)).unwrap();
    check_txs(
        &txs,
        vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
        "600 size best scored txs",
    );

    // 700 size best scored txs
    let txs = tx_pool.package_txs(Some(700)).unwrap();
    check_txs(
        &txs,
        vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
        "700 size best scored txs",
    );

    // 800 size best scored txs
    let txs = tx_pool.package_txs(Some(800)).unwrap();
    check_txs(
        &txs,
        vec![&tx1, &tx2, &tx3, &tx4],
        "800 size best scored txs",
    );

    // none package txs
    let txs = tx_pool.package_txs(Some(30)).unwrap();
    check_txs(&txs, vec![], "none package txs");

    // best scored txs
    let txs = tx_pool.package_txs(None).unwrap();
    check_txs(
        &txs,
        vec![&tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3],
        "best scored txs",
    );
}

#[test]
fn test_package_multi_best_scores() {
    let mut consensus = Consensus::default();
    consensus.genesis_epoch_ext.set_length(5);
    let epoch = consensus.genesis_epoch_ext().clone();
    let (chain_controller, shared) = start_chain(Some(consensus));

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mut parent_header = genesis;
    let mut blocks = vec![];
    for _i in 0..4 {
        let block = gen_block(&parent_header, 11, &epoch);
        chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block");
        parent_header = block.header().to_owned();
        blocks.push(block);
    }

    let tx0 = &blocks[0].transactions()[0];
    let tx1 = build_tx(tx0, &[0], 2);
    let tx2 = build_tx(&tx1, &[0], 2);
    let tx3 = build_tx(&tx2, &[0], 2);
    let tx4 = build_tx(&tx3, &[0], 2);

    let tx2_0 = &blocks[1].transactions()[0];
    let tx2_1 = build_tx(tx2_0, &[0], 2);
    let tx2_2 = build_tx(&tx2_1, &[0], 2);
    let tx2_3 = build_tx(&tx2_2, &[0], 2);
    let tx2_4 = build_tx(&tx2_3, &[0], 2);

    let tx3_0 = &blocks[2].transactions()[0];
    let tx3_1 = build_tx(tx3_0, &[0], 1);

    let tx4_0 = &blocks[3].transactions()[0];
    let tx4_1 = build_tx(tx4_0, &[0], 1);

    let tx_pool = shared.tx_pool_controller();
    let entries = vec![
        TxEntry::dummy_resolve(tx1.clone(), 0, Capacity::shannons(200), 100),
        TxEntry::dummy_resolve(tx2.clone(), 0, Capacity::shannons(200), 100),
        TxEntry::dummy_resolve(tx3.clone(), 0, Capacity::shannons(50), 50),
        TxEntry::dummy_resolve(tx4.clone(), 0, Capacity::shannons(1500), 500),
        TxEntry::dummy_resolve(tx2_1.clone(), 0, Capacity::shannons(150), 100),
        TxEntry::dummy_resolve(tx2_2.clone(), 0, Capacity::shannons(150), 100),
        TxEntry::dummy_resolve(tx2_3.clone(), 0, Capacity::shannons(150), 100),
        TxEntry::dummy_resolve(tx2_4.clone(), 0, Capacity::shannons(150), 100),
        TxEntry::dummy_resolve(tx3_1.clone(), 0, Capacity::shannons(1000), 1000),
        TxEntry::dummy_resolve(tx4_1.clone(), 0, Capacity::shannons(300), 250),
    ];
    tx_pool.plug_entry(entries, PlugTarget::Proposed).unwrap();

    // 250 size best scored txs
    let txs = tx_pool.package_txs(Some(250)).unwrap();
    check_txs(&txs, vec![&tx1, &tx2, &tx3], "250 size best scored txs");

    // 400 size best scored txs
    let txs = tx_pool.package_txs(Some(400)).unwrap();
    check_txs(
        &txs,
        vec![&tx1, &tx2, &tx2_1, &tx2_2],
        "400 size best scored txs",
    );

    // 500 size best scored txs
    let txs = tx_pool.package_txs(Some(500)).unwrap();
    check_txs(
        &txs,
        vec![&tx1, &tx2, &tx2_1, &tx2_2, &tx2_3],
        "500 size best scored txs",
    );

    // 900 size best scored txs
    let txs = tx_pool.package_txs(Some(900)).unwrap();
    check_txs(
        &txs,
        vec![&tx1, &tx2, &tx3, &tx4, &tx2_1],
        "900 size best scored txs",
    );

    // none package txs
    let txs = tx_pool.package_txs(Some(30)).unwrap();
    check_txs(&txs, vec![], "none package txs");

    // best scored txs
    let txs = tx_pool.package_txs(None).unwrap();
    check_txs(
        &txs,
        vec![
            &tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3, &tx2_4, &tx4_1, &tx3_1,
        ],
        "best scored txs",
    );
}

#[test]
fn test_package_low_fee_descendants() {
    let mut consensus = Consensus::default();
    consensus.genesis_epoch_ext.set_length(5);
    let epoch = consensus.genesis_epoch_ext().clone();

    let (chain_controller, shared) = start_chain(Some(consensus));

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mut parent_header = genesis;
    let mut blocks = vec![];
    for _i in 0..4 {
        let block = gen_block(&parent_header, 11, &epoch);
        chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block");
        parent_header = block.header().to_owned();
        blocks.push(block);
    }

    let tx0 = &blocks[0].transactions()[0];
    let tx1 = build_tx(tx0, &[0], 2);
    let tx2 = build_tx(&tx1, &[0], 2);
    let tx3 = build_tx(&tx2, &[0], 2);
    let tx4 = build_tx(&tx3, &[0], 2);
    let tx5 = build_tx(&tx4, &[0], 2);

    let tx_pool = shared.tx_pool_controller();
    let entries = vec![
        TxEntry::dummy_resolve(tx1.clone(), 0, Capacity::shannons(1000), 100),
        TxEntry::dummy_resolve(tx2.clone(), 0, Capacity::shannons(100), 100),
        TxEntry::dummy_resolve(tx3.clone(), 0, Capacity::shannons(100), 100),
        TxEntry::dummy_resolve(tx4.clone(), 0, Capacity::shannons(100), 100),
        TxEntry::dummy_resolve(tx5.clone(), 0, Capacity::shannons(100), 100),
    ];
    tx_pool.plug_entry(entries, PlugTarget::Proposed).unwrap();

    // best scored txs
    let txs = tx_pool.package_txs(None).unwrap();
    check_txs(&txs, vec![&tx1, &tx2, &tx3, &tx4, &tx5], "best scored txs");
}
