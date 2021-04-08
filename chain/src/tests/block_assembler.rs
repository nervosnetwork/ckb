use crate::chain::{ChainController, ChainService};
use crate::tests::util::dummy_network;
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::genesis_dao_data;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_shared::Snapshot;
use ckb_shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_tx_pool::{PlugTarget, TxEntry};
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockBuilder, BlockNumber, BlockView, Capacity, EpochExt, HeaderBuilder, HeaderView,
        TransactionBuilder, TransactionView,
    },
    h256,
    packed::{Block, CellInput, CellOutput, CellOutputBuilder, OutPoint},
    prelude::*,
    H256,
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
    };
    let (shared, mut pack) = builder
        .block_assembler_config(Some(config))
        .build()
        .unwrap();

    let network = dummy_network(&shared);
    pack.take_tx_pool_builder().start(network).unwrap();

    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

lazy_static! {
    static ref BASIC_BLOCK_SIZE: u64 = {
        let (_chain_controller, shared) = start_chain(None);
        let tx_pool = shared.tx_pool_controller();

        let block_template = tx_pool
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
    let tx_pool = shared.tx_pool_controller();

    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();
    let header = block.header();

    let header_verify_result = {
        let snapshot: &Snapshot = &shared.snapshot();
        let header_verifier = HeaderVerifier::new(snapshot, &shared.consensus());
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
        .epoch(epoch.number().pack())
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
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    ::std::env::set_var("FAKETIME", faketime_file.as_os_str());

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
    let tx_pool = shared.tx_pool_controller();

    let mut block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    while (Into::<u64>::into(block_template.number)) != 2 {
        block_template = tx_pool
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

    let tx_pool = shared.tx_pool_controller();

    let mut block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    // block number 3, epoch 0
    while (Into::<u64>::into(block_template.number)) != 3 || block_template.uncles.is_empty() {
        block_template = tx_pool
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

    let mut block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    while (Into::<u64>::into(block_template.number)) != 4 {
        block_template = tx_pool
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

    let mut block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    while (Into::<u64>::into(block_template.number)) != 5 {
        block_template = tx_pool
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }
    // block number 5, epoch 1, block_template should not include last epoch uncles
    assert!(block_template.uncles.is_empty());
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

fn check_txs(block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>) {
    assert_eq!(
        block_template
            .transactions
            .iter()
            .map(|tx| format!("{}", tx.hash))
            .collect::<Vec<_>>(),
        expect_txs
            .iter()
            .map(|tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
            .collect::<Vec<_>>()
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
    let block_template = tx_pool
        .get_block_template(Some(300 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3]);

    // 400 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(400 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3, &tx1]);

    // 500 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(500 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2]);

    // 600 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(600 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(
        &block_template,
        vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
    );

    // 700 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(700 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(
        &block_template,
        vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
    );

    // 800 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(800 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4]);

    // none package txs
    let block_template = tx_pool
        .get_block_template(Some(30 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![]);

    // best scored txs
    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    check_txs(
        &block_template,
        vec![&tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3],
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
    let block_template = tx_pool
        .get_block_template(Some(250 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx1, &tx2, &tx3]);

    // 400 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(400 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx1, &tx2, &tx2_1, &tx2_2]);

    // 500 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(500 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx1, &tx2, &tx2_1, &tx2_2, &tx2_3]);

    // 900 size best scored txs
    let block_template = tx_pool
        .get_block_template(Some(900 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4, &tx2_1]);

    // none package txs
    let block_template = tx_pool
        .get_block_template(Some(30 + *BASIC_BLOCK_SIZE), None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![]);

    // best scored txs
    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    check_txs(
        &block_template,
        vec![
            &tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3, &tx2_4, &tx4_1, &tx3_1,
        ],
    );
}

#[test]
fn test_package_low_fee_decendants() {
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
    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4, &tx5]);
}

#[test]
fn test_package_txs_lower_than_min_fee_rate() {
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
            .unwrap();
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
        TxEntry::dummy_resolve(tx2, 0, Capacity::shannons(80), 100),
        TxEntry::dummy_resolve(tx3, 0, Capacity::shannons(50), 100),
        TxEntry::dummy_resolve(tx4, 0, Capacity::shannons(20), 100),
        TxEntry::dummy_resolve(tx5, 0, Capacity::shannons(0), 100),
    ];
    tx_pool.plug_entry(entries, PlugTarget::Proposed).unwrap();

    let check_txs = |block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>| {
        assert_eq!(
            block_template
                .transactions
                .iter()
                .map(|tx| format!("{}", tx.hash))
                .collect::<Vec<_>>(),
            expect_txs
                .iter()
                .map(|tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
                .collect::<Vec<_>>()
        );
    };
    // best scored txs
    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();
    check_txs(&block_template, vec![&tx1]);
}
