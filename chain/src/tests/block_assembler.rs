use crate::chain::ChainController;
use crate::chain::ChainService;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::genesis_dao_data;
use ckb_jsonrpc_types::{JsonBytes, ScriptHashType};
use ckb_pow::Pow;
use ckb_shared::shared::Shared;
use ckb_shared::shared::SharedBuilder;
use ckb_shared::Snapshot;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_tx_pool::BlockAssemblerConfig;
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
use ckb_verification::{BlockVerifier, HeaderResolverWrapper, HeaderVerifier, Verifier};
use futures::future::Future;
use std::sync::Arc;

const BASIC_BLOCK_SIZE: u64 = 706;

fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared) {
    let mut builder = SharedBuilder::default();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let config = BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: vec![],
        data: JsonBytes::default(),
        hash_type: ScriptHashType::Data,
    };
    let (shared, table) = builder
        .block_assembler_config(Some(config))
        .build()
        .unwrap();

    let chain_service = ChainService::new(shared.clone(), table);
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

#[test]
fn test_get_block_template() {
    let (_chain_controller, shared) = start_chain(None);
    let config = BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: vec![],
        data: JsonBytes::default(),
        hash_type: ScriptHashType::Data,
    };

    let tx_pool = shared.tx_pool_controller();

    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .wait()
        .unwrap()
        .unwrap();

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();
    let header = block.header();

    let resolver = HeaderResolverWrapper::new(&header, shared.store(), shared.consensus());
    let header_verify_result = {
        let snapshot: &Snapshot = &shared.snapshot();
        let header_verifier = HeaderVerifier::new(snapshot, Pow::Dummy.engine());
        header_verifier.verify(&resolver)
    };
    assert!(header_verify_result.is_ok());

    let block_verify = BlockVerifier::new(shared.consensus());
    assert!(block_verify.verify(&block).is_ok());
}

fn gen_block(parent_header: &HeaderView, nonce: u64, epoch: &EpochExt) -> BlockView {
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
        .difficulty(epoch.difficulty().clone().pack())
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
    let hash0_0: H256 = block0_0.hash().unpack();
    let hash0_1: H256 = block0_1.hash().unpack();
    let (block0_0, block0_1) = if hash0_0 < hash0_1 {
        (block0_1, block0_0)
    } else {
        (block0_0, block0_1)
    };

    let last_epoch = epoch.clone();
    let epoch = shared
        .next_epoch_ext(&last_epoch, &block0_1.header())
        .unwrap_or(last_epoch);

    let block1_1 = gen_block(&block0_1.header(), 10, &epoch);

    chain_controller
        .process_block(Arc::new(block0_1.clone()), false)
        .unwrap();
    chain_controller
        .process_block(Arc::new(block0_0.clone()), false)
        .unwrap();
    chain_controller
        .process_block(Arc::new(block1_1.clone()), false)
        .unwrap();

    let tx_pool = shared.tx_pool_controller();

    // block number 3, epoch 0
    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .wait()
        .unwrap()
        .unwrap();
    assert_eq!(block_template.uncles[0].hash, block0_0.hash().unpack());

    let last_epoch = epoch.clone();
    let epoch = shared
        .next_epoch_ext(&last_epoch, &block1_1.header())
        .unwrap_or(last_epoch);

    let block2_1 = gen_block(&block1_1.header(), 10, &epoch);
    chain_controller
        .process_block(Arc::new(block2_1.clone()), false)
        .unwrap();

    let block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .wait()
        .unwrap()
        .unwrap();
    // block number 4, epoch 0, uncles should retained
    assert_eq!(block_template.uncles[0].hash, block0_0.hash().unpack());

    let last_epoch = epoch.clone();
    let epoch = shared
        .next_epoch_ext(&last_epoch, &block2_1.header())
        .unwrap_or(last_epoch);

    let block3_1 = gen_block(&block2_1.header(), 10, &epoch);
    chain_controller
        .process_block(Arc::new(block3_1.clone()), false)
        .unwrap();

    // let block_template = tx_pool.get_block_template(None, None, None).unwrap().wait().unwrap().unwrap();
    // // block number 5, epoch 1, block_template should not include last epoch uncles
    // assert!(block_template.uncles.is_empty());
}

// fn build_tx(parent_tx: &TransactionView, inputs: &[u32], outputs_len: usize) -> TransactionView {
//     let per_output_capacity =
//         Capacity::shannons(parent_tx.outputs_capacity().unwrap().as_u64() / outputs_len as u64);
//     TransactionBuilder::default()
//         .inputs(
//             inputs
//                 .iter()
//                 .map(|index| CellInput::new(OutPoint::new(parent_tx.hash().to_owned(), *index), 0)),
//         )
//         .outputs(
//             (0..outputs_len)
//                 .map(|_| {
//                     CellOutputBuilder::default()
//                         .capacity(per_output_capacity.pack())
//                         .build()
//                 })
//                 .collect::<Vec<CellOutput>>(),
//         )
//         .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
//         .build()
// }

// #[test]
// fn test_package_basic() {
//     let mut consensus = Consensus::default();
//     consensus.genesis_epoch_ext.set_length(5);
//     let epoch = consensus.genesis_epoch_ext().clone();

//     let (chain_controller, shared) = start_chain(Some(consensus));

//     let genesis = shared
//         .store()
//         .get_block_header(&shared.store().get_block_hash(0).unwrap())
//         .unwrap();
//     let mut parent_header = genesis.to_owned();
//     let mut blocks = vec![];
//     for _i in 0..4 {
//         let block = gen_block(&parent_header, 11, &epoch);
//         chain_controller
//             .process_block(Arc::new(block.clone()), false)
//             .expect("process block");
//         parent_header = block.header().to_owned();
//         blocks.push(block);
//     }

//     let tx0 = &blocks[0].transactions()[0];
//     let tx1 = build_tx(tx0, &[0], 2);
//     let tx2 = build_tx(&tx1, &[0], 2);
//     let tx3 = build_tx(&tx2, &[0], 2);
//     let tx4 = build_tx(&tx3, &[0], 2);

//     let tx2_0 = &blocks[1].transactions()[0];
//     let tx2_1 = build_tx(tx2_0, &[0], 2);
//     let tx2_2 = build_tx(&tx2_1, &[0], 2);
//     let tx2_3 = build_tx(&tx2_2, &[0], 2);

//     {

//         for (tx, fee, cycles, size) in &[
//             (&tx1, 100, 0, 100),
//             (&tx2, 100, 0, 100),
//             (&tx3, 100, 0, 100),
//             (&tx4, 1500, 0, 500),
//             (&tx2_1, 150, 0, 100),
//             (&tx2_2, 150, 0, 100),
//             (&tx2_3, 150, 0, 100),
//         ] {
//             tx_pool.add_proposed(
//                 *cycles,
//                 Capacity::shannons(*fee),
//                 *size,
//                 (*tx).to_owned(),
//                 vec![],
//             );
//         }
//     }

//     let check_txs = |block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>| {
//         assert_eq!(
//             block_template
//                 .transactions
//                 .iter()
//                 .map(|tx| format!("{}", tx.hash))
//                 .collect::<Vec<_>>(),
//             expect_txs
//                 .iter()
//                 .map(|tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
//                 .collect::<Vec<_>>()
//         );
//     };

//     // 300 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(300 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3]);

//     // 400 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(400 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3, &tx1]);

//     // 500 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(500 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2]);

//     // 600 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(600 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(
//         &block_template,
//         vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
//     );

//     // 700 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(700 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(
//         &block_template,
//         vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
//     );

//     // 800 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(800 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4]);

//     // none package txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(30 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![]);

//     // best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(None, None, None)
//         .unwrap();
//     check_txs(
//         &block_template,
//         vec![&tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3],
//     );
// }

// #[test]
// fn test_package_multi_best_scores() {
//     let mut consensus = Consensus::default();
//     consensus.genesis_epoch_ext.set_length(5);
//     let epoch = consensus.genesis_epoch_ext().clone();

//     let (chain_controller, shared) = start_chain(Some(consensus));

//     let genesis = shared
//         .store()
//         .get_block_header(&shared.store().get_block_hash(0).unwrap())
//         .unwrap();
//     let mut parent_header = genesis.to_owned();
//     let mut blocks = vec![];
//     for _i in 0..4 {
//         let block = gen_block(&parent_header, 11, &epoch);
//         chain_controller
//             .process_block(Arc::new(block.clone()), false)
//             .expect("process block");
//         parent_header = block.header().to_owned();
//         blocks.push(block);
//     }

//     let tx0 = &blocks[0].transactions()[0];
//     let tx1 = build_tx(tx0, &[0], 2);
//     let tx2 = build_tx(&tx1, &[0], 2);
//     let tx3 = build_tx(&tx2, &[0], 2);
//     let tx4 = build_tx(&tx3, &[0], 2);

//     let tx2_0 = &blocks[1].transactions()[0];
//     let tx2_1 = build_tx(tx2_0, &[0], 2);
//     let tx2_2 = build_tx(&tx2_1, &[0], 2);
//     let tx2_3 = build_tx(&tx2_2, &[0], 2);
//     let tx2_4 = build_tx(&tx2_3, &[0], 2);

//     let tx3_0 = &blocks[2].transactions()[0];
//     let tx3_1 = build_tx(tx3_0, &[0], 1);

//     let tx4_0 = &blocks[3].transactions()[0];
//     let tx4_1 = build_tx(tx4_0, &[0], 1);

//     {
//         let mut tx_pool = shared.try_lock_tx_pool();
//         for (tx, fee, cycles, size) in &[
//             (&tx1, 200, 0, 100),
//             (&tx2, 200, 0, 100),
//             (&tx3, 50, 0, 50),
//             (&tx4, 1500, 0, 500),
//             (&tx2_1, 150, 0, 100),
//             (&tx2_2, 150, 0, 100),
//             (&tx2_3, 150, 0, 100),
//             (&tx2_4, 150, 0, 100),
//             (&tx3_1, 1000, 0, 1000),
//             (&tx4_1, 300, 0, 250),
//         ] {
//             tx_pool.add_proposed(
//                 *cycles,
//                 Capacity::shannons(*fee),
//                 *size,
//                 (*tx).to_owned(),
//                 vec![],
//             );
//         }
//     }

//     let check_txs = |block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>| {
//         assert_eq!(
//             block_template
//                 .transactions
//                 .iter()
//                 .map(|tx| format!("{}", tx.hash))
//                 .collect::<Vec<_>>(),
//             expect_txs
//                 .iter()
//                 .map(|tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
//                 .collect::<Vec<_>>()
//         );
//     };

//     // 250 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(250 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx1, &tx2, &tx3]);

//     // 400 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(400 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx1, &tx2, &tx2_1, &tx2_2]);

//     // 500 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(500 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx1, &tx2, &tx2_1, &tx2_2, &tx2_3]);

//     // 900 size best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(900 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4, &tx2_1]);

//     // none package txs
//     let block_template = block_assembler_controller
//         .get_block_template(Some(30 + BASIC_BLOCK_SIZE), None, None)
//         .unwrap();
//     check_txs(&block_template, vec![]);

//     // best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(None, None, None)
//         .unwrap();
//     check_txs(
//         &block_template,
//         vec![
//             &tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3, &tx2_4, &tx4_1, &tx3_1,
//         ],
//     );
// }

// #[test]
// fn test_package_zero_fee_txs() {
//     let mut consensus = Consensus::default();
//     consensus.genesis_epoch_ext.set_length(5);
//     let epoch = consensus.genesis_epoch_ext().clone();

//     let (chain_controller, shared, notify) = start_chain(Some(consensus));
//     let config = BlockAssemblerConfig {
//         code_hash: h256!("0x0"),
//         args: vec![],
//         data: JsonBytes::default(),
//         hash_type: ScriptHashType::Data,
//     };
//     let block_assembler = setup_block_assembler(shared.clone(), config);
//     let block_assembler_controller = block_assembler.start(Some("test"), &notify.clone());

//     let genesis = shared
//         .store()
//         .get_block_header(&shared.store().get_block_hash(0).unwrap())
//         .unwrap();
//     let mut parent_header = genesis.to_owned();
//     let mut blocks = vec![];
//     for _i in 0..4 {
//         let block = gen_block(&parent_header, 11, &epoch);
//         chain_controller
//             .process_block(Arc::new(block.clone()), false)
//             .expect("process block");
//         parent_header = block.header().to_owned();
//         blocks.push(block);
//     }

//     let tx0 = &blocks[0].transactions()[0];
//     let tx1 = build_tx(tx0, &[0], 2);
//     let tx2 = build_tx(&tx1, &[0], 2);
//     let tx3 = build_tx(&tx2, &[0], 2);
//     let tx4 = build_tx(&tx3, &[0], 2);
//     let tx5 = build_tx(&tx4, &[0], 2);

//     {
//         let mut tx_pool = shared.try_lock_tx_pool();
//         for (tx, fee, cycles, size) in &[
//             (&tx1, 1000, 0, 100),
//             (&tx2, 0, 0, 100),
//             (&tx3, 0, 0, 100),
//             (&tx4, 0, 0, 100),
//             (&tx5, 0, 0, 100),
//         ] {
//             tx_pool.add_proposed(
//                 *cycles,
//                 Capacity::shannons(*fee),
//                 *size,
//                 (*tx).to_owned(),
//                 vec![],
//             );
//         }
//     }

//     let check_txs = |block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>| {
//         assert_eq!(
//             block_template
//                 .transactions
//                 .iter()
//                 .map(|tx| format!("{}", tx.hash))
//                 .collect::<Vec<_>>(),
//             expect_txs
//                 .iter()
//                 .map(|&tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
//                 .collect::<Vec<_>>()
//         );
//     };
//     // best scored txs
//     let block_template = block_assembler_controller
//         .get_block_template(None, None, None)
//         .unwrap();
//     check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4, &tx5]);
// }
