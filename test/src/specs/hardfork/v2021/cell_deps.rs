use crate::{
    util::{
        cell::gen_spendable,
        check::{assert_epoch_should_be, assert_epoch_should_less_than, is_transaction_committed},
        mining::{mine, mine_until_bool, mine_until_epoch},
    },
    utils::assert_send_transaction_fail,
    Node, Spec,
};
use ckb_jsonrpc_types as rpc;
use ckb_logger::{debug, info};
use ckb_types::{
    core::{Capacity, DepType, ScriptHashType, TransactionBuilder, TransactionView},
    packed,
    prelude::*,
};
use std::fmt;

const GENESIS_EPOCH_LENGTH: u64 = 10;
const CKB2021_START_EPOCH: u64 = 10;

const TEST_CASES_COUNT: usize = (8 + 4) * 3;
const CELL_DEPS_COUNT: usize = 2 + 3 + 2;
const INITIAL_INPUTS_COUNT: usize = 2 + CELL_DEPS_COUNT + TEST_CASES_COUNT;

pub struct DuplicateCellDepsForDataHashTypeLockScript;
pub struct DuplicateCellDepsForDataHashTypeTypeScript;
pub struct DuplicateCellDepsForTypeHashTypeLockScript;
pub struct DuplicateCellDepsForTypeHashTypeTypeScript;

struct NewScript {
    data: packed::Bytes,
    cell_dep: packed::CellDep,
    data_hash: packed::Byte32,
    type_hash: packed::Byte32,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedResult {
    ShouldBePassed,
    DuplicateCellDeps,
    MultipleMatchesLock,
    MultipleMatchesType,
}

const PASS: ExpectedResult = ExpectedResult::ShouldBePassed;
const DUP: ExpectedResult = ExpectedResult::DuplicateCellDeps;
const MML: ExpectedResult = ExpectedResult::MultipleMatchesLock;
const MMT: ExpectedResult = ExpectedResult::MultipleMatchesType;

// For all:
//      - code1 and code2 are cell deps with same data
//      - dep_group1 and dep_group2 are cell deps which point to different code cell deps with same data
//      - dep_group2 and dep_group2_copy are cell deps which point to same code cell deps
//  For type hash type only:
//      - code3 has same type with code1 (code2) but different data
//      - dep_group3 has same type with dep_group1 (dep_group2, dep_group2_copy) but different data
struct CellDepSet {
    code1: packed::CellDep,
    code2: packed::CellDep,
    dep_group1: packed::CellDep,
    dep_group2: packed::CellDep,
    dep_group2_copy: packed::CellDep,
    code3: packed::CellDep,
    dep_group3: packed::CellDep,
}

struct DuplicateCellDepsTestRunner {
    tag: &'static str,
}

impl Spec for DuplicateCellDepsForDataHashTypeLockScript {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;
        let runner = DuplicateCellDepsTestRunner::new("data-hash-type/lock-script");
        let mut original_inputs = gen_spendable(node, INITIAL_INPUTS_COUNT)
            .into_iter()
            .map(|input| packed::CellInput::new(input.out_point, 0));
        let script1 = NewScript::new_with_id(node, 1, &mut original_inputs, None);
        let mut inputs = {
            let txs = original_inputs.by_ref().take(TEST_CASES_COUNT).collect();
            runner.use_new_data_script_replace_lock_script(node, txs, &script1)
        };
        let deps = runner.create_cell_dep_set(node, &mut original_inputs, &script1, None);
        let tb = TransactionView::new_advanced_builder();
        {
            info!("CKB v2019:");
            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
        }
        assert_epoch_should_less_than(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine_until_epoch(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        {
            info!("CKB v2019 (boundary):");
            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);
        }
        mine(node, 1);
        {
            info!("CKB v2021:");
            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_pr_0222 = Some(CKB2021_START_EPOCH);
        }
    }
}

impl Spec for DuplicateCellDepsForDataHashTypeTypeScript {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;
        let runner = DuplicateCellDepsTestRunner::new("data-hash-type/type-script");
        let mut original_inputs = gen_spendable(node, INITIAL_INPUTS_COUNT)
            .into_iter()
            .map(|input| packed::CellInput::new(input.out_point, 0));
        let script1 = NewScript::new_with_id(node, 1, &mut original_inputs, None);
        let mut inputs = {
            let txs = original_inputs.by_ref().take(TEST_CASES_COUNT).collect();
            runner.add_new_data_script_as_type_script(node, txs, &script1)
        };
        let deps = runner.create_cell_dep_set(node, &mut original_inputs, &script1, None);
        let tb = TransactionView::new_advanced_builder().cell_dep(node.always_success_cell_dep());
        {
            info!("CKB v2019:");
            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
        }
        assert_epoch_should_less_than(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine_until_epoch(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        {
            info!("CKB v2019 (boundary):");
            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);
        }
        mine(node, 1);
        {
            info!("CKB v2021:");
            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_pr_0222 = Some(CKB2021_START_EPOCH);
        }
    }
}

impl Spec for DuplicateCellDepsForTypeHashTypeLockScript {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;
        let runner = DuplicateCellDepsTestRunner::new("type-hash-type/lock-script");
        let mut original_inputs = gen_spendable(node, INITIAL_INPUTS_COUNT)
            .into_iter()
            .map(|input| packed::CellInput::new(input.out_point, 0));
        let script0 = NewScript::new_with_id(node, 0, &mut original_inputs, None);
        let script1 = NewScript::new_with_id(node, 1, &mut original_inputs, Some(&script0));
        let mut inputs = {
            let txs = original_inputs.by_ref().take(TEST_CASES_COUNT).collect();
            runner.use_new_data_script_replace_type_script(node, txs, &script1)
        };
        let deps = runner.create_cell_dep_set(node, &mut original_inputs, &script1, Some(&script0));
        let tb = TransactionView::new_advanced_builder();
        {
            info!("CKB v2019:");
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, MML);

            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);

            // Type hash type only
            runner.test_same_type_not_same_data_code_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_hybrid_type_v1(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_hybrid_type_v2(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MML);
        }
        assert_epoch_should_less_than(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine_until_epoch(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        {
            info!("CKB v2019 (boundary):");
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, MML);

            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);
            // Type hash type only
            runner.test_same_type_not_same_data_code_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_hybrid_type_v1(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_hybrid_type_v2(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MML);
        }
        assert_epoch_should_be(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine(node, 1);
        {
            info!("CKB v2021:");
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, PASS);

            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);

            // Type hash type only
            runner.test_same_type_not_same_data_code_type(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_hybrid_type_v1(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_hybrid_type_v2(node, &deps, &mut inputs, &tb, MML);
            runner.test_same_type_not_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MML);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_pr_0222 = Some(CKB2021_START_EPOCH);
        }
    }
}

impl Spec for DuplicateCellDepsForTypeHashTypeTypeScript {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let epoch_length = GENESIS_EPOCH_LENGTH;
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;
        let runner = DuplicateCellDepsTestRunner::new("type-hash-type/type-script");
        let mut original_inputs = gen_spendable(node, INITIAL_INPUTS_COUNT)
            .into_iter()
            .map(|input| packed::CellInput::new(input.out_point, 0));
        let script0 = NewScript::new_with_id(node, 0, &mut original_inputs, None);
        let script1 = NewScript::new_with_id(node, 1, &mut original_inputs, Some(&script0));
        let mut inputs = {
            let txs = original_inputs.by_ref().take(TEST_CASES_COUNT).collect();
            runner.add_new_type_script_as_type_script(node, txs, &script1)
        };
        let deps = runner.create_cell_dep_set(node, &mut original_inputs, &script1, Some(&script0));
        let tb = TransactionView::new_advanced_builder().cell_dep(node.always_success_cell_dep());
        {
            info!("CKB v2019:");
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, MMT);

            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            // Type hash type only
            runner.test_same_type_not_same_data_code_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_hybrid_type_v1(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_hybrid_type_v2(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MMT);
        }
        assert_epoch_should_less_than(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine_until_epoch(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        {
            info!("CKB v2019 (boundary):");
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, MMT);

            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);
            // Type hash type only
            runner.test_same_type_not_same_data_code_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_hybrid_type_v1(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_hybrid_type_v2(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MMT);
        }
        assert_epoch_should_be(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine(node, 1);
        {
            info!("CKB v2021:");
            runner.test_same_data_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_hybrid_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_data_dep_group_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_same_point_dep_group_type(node, &deps, &mut inputs, &tb, PASS);

            runner.test_duplicate_code_type(node, &deps, &mut inputs, &tb, DUP);
            runner.test_duplicate_dep_group_type(node, &deps, &mut inputs, &tb, DUP);

            runner.test_single_code_type(node, &deps, &mut inputs, &tb, PASS);
            runner.test_single_dep_group_type(node, &deps, &mut inputs, &tb, PASS);

            // Type hash type only
            runner.test_same_type_not_same_data_code_type(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_hybrid_type_v1(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_hybrid_type_v2(node, &deps, &mut inputs, &tb, MMT);
            runner.test_same_type_not_same_data_dep_group_type(node, &deps, &mut inputs, &tb, MMT);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_pr_0222 = Some(CKB2021_START_EPOCH);
        }
    }
}

impl NewScript {
    fn new_with_id(
        node: &Node,
        id: u8,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        type_script_opt: Option<&Self>,
    ) -> Self {
        let original_data = node.always_success_raw_data();
        let data = packed::Bytes::new_builder()
            .extend(original_data.as_ref().iter().map(|x| (*x).into()))
            .push(id.into())
            .build();
        let tx = Self::deploy(node, &data, inputs, type_script_opt);
        let cell_dep = packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(tx.hash(), 0))
            .dep_type(DepType::Code.into())
            .build();
        let data_hash = packed::CellOutput::calc_data_hash(&data.raw_data());
        let type_hash = tx
            .output(0)
            .unwrap()
            .type_()
            .to_opt()
            .unwrap()
            .calc_script_hash();
        Self {
            data,
            cell_dep,
            data_hash,
            type_hash,
        }
    }

    fn deploy(
        node: &Node,
        data: &packed::Bytes,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        type_script_opt: Option<&Self>,
    ) -> TransactionView {
        let (type_script, tx_template) = if let Some(script) = type_script_opt {
            (
                script.as_data_script(),
                TransactionView::new_advanced_builder().cell_dep(script.cell_dep()),
            )
        } else {
            (
                node.always_success_script(),
                TransactionView::new_advanced_builder(),
            )
        };
        let cell_input = inputs.next().unwrap();
        let cell_output = packed::CellOutput::new_builder()
            .type_(Some(type_script).pack())
            .build_exact_capacity(Capacity::bytes(data.len()).unwrap())
            .unwrap();
        let tx = tx_template
            .cell_dep(node.always_success_cell_dep())
            .input(cell_input)
            .output(cell_output)
            .output_data(data.clone())
            .build();
        node.submit_transaction(&tx);
        mine_until_bool(node, || is_transaction_committed(node, &tx));
        tx
    }

    fn data(&self) -> packed::Bytes {
        self.data.clone()
    }

    fn cell_dep(&self) -> packed::CellDep {
        self.cell_dep.clone()
    }

    fn as_data_script(&self) -> packed::Script {
        packed::Script::new_builder()
            .code_hash(self.data_hash.clone())
            .hash_type(ScriptHashType::Data(0).into())
            .build()
    }

    fn as_type_script(&self) -> packed::Script {
        packed::Script::new_builder()
            .code_hash(self.type_hash.clone())
            .hash_type(ScriptHashType::Type.into())
            .build()
    }
}

impl fmt::Display for ExpectedResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ShouldBePassed => write!(f, "allowed"),
            _ => write!(f, "not allowed"),
        }
    }
}

impl ExpectedResult {
    fn error_message(self) -> Option<&'static str> {
        match self {
            Self::ShouldBePassed => None,
            Self::DuplicateCellDeps => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Transaction(DuplicateCellDeps(",
            ),
            Self::MultipleMatchesLock => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError \
                 { source: Inputs[0].Lock, cause: MultipleMatches })",
            ),
            Self::MultipleMatchesType => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError \
                 { source: Inputs[0].Type, cause: MultipleMatches })",
            ),
        }
    }
}

impl DuplicateCellDepsTestRunner {
    fn new(tag: &'static str) -> Self {
        Self { tag }
    }

    fn submit_transaction_until_committed(&self, node: &Node, tx: &TransactionView) {
        debug!(
            "[{}] >>> >>> submit: submit transaction {:#x}.",
            self.tag,
            tx.hash()
        );
        node.submit_transaction(tx);
        mine_until_bool(node, || is_transaction_committed(node, tx));
    }
}

// Convert Lock Script or Type Script
impl DuplicateCellDepsTestRunner {
    fn create_initial_inputs(
        &self,
        node: &Node,
        txs: Vec<TransactionView>,
    ) -> impl Iterator<Item = packed::CellInput> {
        for tx in &txs {
            node.rpc_client().send_transaction(tx.data().into());
        }
        mine_until_bool(node, || {
            txs.iter().all(|tx| is_transaction_committed(node, &tx))
        });
        txs.into_iter().map(|tx| {
            let out_point = packed::OutPoint::new(tx.hash(), 0);
            packed::CellInput::new(out_point, 0)
        })
    }

    fn get_previous_output(node: &Node, cell_input: &packed::CellInput) -> rpc::CellOutput {
        let previous_output = cell_input.previous_output();
        let previous_output_index: usize = previous_output.index().unpack();
        node.rpc_client()
            .get_transaction(previous_output.tx_hash())
            .unwrap()
            .transaction
            .inner
            .outputs[previous_output_index]
            .clone()
    }

    fn use_new_data_script_replace_lock_script(
        &self,
        node: &Node,
        inputs: Vec<packed::CellInput>,
        new_script: &NewScript,
    ) -> impl Iterator<Item = packed::CellInput> {
        let txs = inputs
            .into_iter()
            .map(|cell_input| {
                let input_cell = Self::get_previous_output(node, &cell_input);
                let cell_output = packed::CellOutput::new_builder()
                    .capacity((input_cell.capacity.value() - 1).pack())
                    .lock(new_script.as_data_script())
                    .build();
                TransactionView::new_advanced_builder()
                    .cell_dep(node.always_success_cell_dep())
                    .cell_dep(new_script.cell_dep())
                    .input(cell_input)
                    .output(cell_output)
                    .output_data(Default::default())
                    .build()
            })
            .collect::<Vec<_>>();
        self.create_initial_inputs(node, txs)
    }

    fn add_new_data_script_as_type_script(
        &self,
        node: &Node,
        inputs: Vec<packed::CellInput>,
        new_script: &NewScript,
    ) -> impl Iterator<Item = packed::CellInput> {
        let txs = inputs
            .into_iter()
            .map(|cell_input| {
                let input_cell = Self::get_previous_output(node, &cell_input);
                let cell_output = packed::CellOutput::new_builder()
                    .capacity((input_cell.capacity.value() - 1).pack())
                    .lock(node.always_success_script())
                    .type_(Some(new_script.as_data_script()).pack())
                    .build();
                TransactionView::new_advanced_builder()
                    .cell_dep(node.always_success_cell_dep())
                    .cell_dep(new_script.cell_dep())
                    .input(cell_input)
                    .output(cell_output)
                    .output_data(Default::default())
                    .build()
            })
            .collect::<Vec<_>>();
        self.create_initial_inputs(node, txs)
    }

    fn use_new_data_script_replace_type_script(
        &self,
        node: &Node,
        inputs: Vec<packed::CellInput>,
        new_script: &NewScript,
    ) -> impl Iterator<Item = packed::CellInput> {
        let txs = inputs
            .into_iter()
            .map(|cell_input| {
                let input_cell = Self::get_previous_output(node, &cell_input);
                let cell_output = packed::CellOutput::new_builder()
                    .capacity((input_cell.capacity.value() - 1).pack())
                    .lock(new_script.as_type_script())
                    .build();
                TransactionView::new_advanced_builder()
                    .cell_dep(node.always_success_cell_dep())
                    .cell_dep(new_script.cell_dep())
                    .input(cell_input)
                    .output(cell_output)
                    .output_data(Default::default())
                    .build()
            })
            .collect::<Vec<_>>();
        self.create_initial_inputs(node, txs)
    }

    fn add_new_type_script_as_type_script(
        &self,
        node: &Node,
        inputs: Vec<packed::CellInput>,
        new_script: &NewScript,
    ) -> impl Iterator<Item = packed::CellInput> {
        let txs = inputs
            .into_iter()
            .map(|cell_input| {
                let input_cell = Self::get_previous_output(node, &cell_input);
                let cell_output = packed::CellOutput::new_builder()
                    .capacity((input_cell.capacity.value() - 1).pack())
                    .lock(node.always_success_script())
                    .type_(Some(new_script.as_type_script()).pack())
                    .build();
                TransactionView::new_advanced_builder()
                    .cell_dep(node.always_success_cell_dep())
                    .cell_dep(new_script.cell_dep())
                    .input(cell_input)
                    .output(cell_output)
                    .output_data(Default::default())
                    .build()
            })
            .collect::<Vec<_>>();
        self.create_initial_inputs(node, txs)
    }
}

// Create All Cell Deps for Test
impl DuplicateCellDepsTestRunner {
    fn create_cell_dep_set(
        &self,
        node: &Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        script: &NewScript,
        type_script_opt: Option<&NewScript>,
    ) -> CellDepSet {
        let code_txs = {
            let tx_template = {
                let script_output = if let Some(type_script) = type_script_opt {
                    packed::CellOutput::new_builder()
                        .type_(Some(type_script.as_data_script()).pack())
                } else {
                    packed::CellOutput::new_builder()
                }
                .build_exact_capacity(Capacity::bytes(script.data().len()).unwrap())
                .unwrap();
                if let Some(type_script) = type_script_opt {
                    TransactionView::new_advanced_builder().cell_dep(type_script.cell_dep())
                } else {
                    TransactionView::new_advanced_builder()
                }
                .output(script_output)
                .output_data(script.data())
            };
            self.create_transactions_as_code_type_cell_deps(node, inputs, &tx_template)
        };

        let dep_group_txs = {
            let tx_template = TransactionView::new_advanced_builder();
            self.create_transactions_as_depgroup_type_cell_deps(
                node,
                inputs,
                &tx_template,
                &code_txs,
            )
        };
        let incorrect_opt = type_script_opt.map(|type_script| {
            self.create_transactions_as_incorrect_cell_deps(node, inputs, type_script)
        });
        self.combine_cell_deps(code_txs, dep_group_txs, incorrect_opt)
    }

    fn create_transactions_as_code_type_cell_deps(
        &self,
        node: &Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
    ) -> (TransactionView, TransactionView) {
        info!(
            "[{}] >>> warm up: create 2 transactions as code-type cell deps.",
            self.tag
        );
        let tx_template = tx_template.clone().cell_dep(node.always_success_cell_dep());
        let dep1_tx = tx_template.clone().input(inputs.next().unwrap()).build();
        let dep2_tx = tx_template.input(inputs.next().unwrap()).build();
        self.submit_transaction_until_committed(node, &dep1_tx);
        self.submit_transaction_until_committed(node, &dep2_tx);
        (dep1_tx, dep2_tx)
    }

    fn create_transactions_as_depgroup_type_cell_deps(
        &self,
        node: &Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        code_txs: &(TransactionView, TransactionView),
    ) -> (TransactionView, TransactionView, TransactionView) {
        info!(
            "[{}] >>> warm up: create 3 transactions as depgroup-type cell deps.",
            self.tag
        );
        let (ref dep1_tx, ref dep2_tx) = code_txs;
        let tx_template = tx_template.clone().cell_dep(node.always_success_cell_dep());
        let dep1_op = packed::OutPoint::new(dep1_tx.hash(), 0);
        let dep2_op = packed::OutPoint::new(dep2_tx.hash(), 0);
        let dep3_data = vec![dep1_op].pack().as_bytes().pack();
        let dep4_data = vec![dep2_op].pack().as_bytes().pack();
        let dep3_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::bytes(dep3_data.len()).unwrap())
            .unwrap();
        let dep4_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::bytes(dep4_data.len()).unwrap())
            .unwrap();
        let dep3_tx = tx_template
            .clone()
            .input(inputs.next().unwrap())
            .output(dep3_output)
            .output_data(dep3_data)
            .build();
        let dep4_tx = tx_template
            .clone()
            .input(inputs.next().unwrap())
            .output(dep4_output.clone())
            .output_data(dep4_data.clone())
            .build();
        let dep4b_tx = tx_template
            .input(inputs.next().unwrap())
            .output(dep4_output)
            .output_data(dep4_data)
            .build();
        self.submit_transaction_until_committed(node, &dep3_tx);
        self.submit_transaction_until_committed(node, &dep4_tx);
        self.submit_transaction_until_committed(node, &dep4b_tx);
        (dep3_tx, dep4_tx, dep4b_tx)
    }

    fn create_transactions_as_incorrect_cell_deps(
        &self,
        node: &Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        type_script: &NewScript,
    ) -> (TransactionView, TransactionView) {
        info!(
            "[{}] >>> warm up: create 2 transactions as incorrect cell deps.",
            self.tag
        );
        let original_data = node.always_success_raw_data();
        let dep5_data = packed::Bytes::new_builder()
            .extend(original_data.as_ref().iter().map(|x| (*x).into()))
            .build();
        let dep5_output = packed::CellOutput::new_builder()
            .type_(Some(type_script.as_data_script()).pack())
            .build_exact_capacity(Capacity::bytes(dep5_data.len()).unwrap())
            .unwrap();
        let dep5_tx = TransactionView::new_advanced_builder()
            .cell_dep(node.always_success_cell_dep())
            .cell_dep(type_script.cell_dep())
            .input(inputs.next().unwrap())
            .output(dep5_output)
            .output_data(dep5_data)
            .build();
        let dep5_op = packed::OutPoint::new(dep5_tx.hash(), 0);
        let dep6_data = vec![dep5_op].pack().as_bytes().pack();
        let dep6_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::bytes(dep6_data.len()).unwrap())
            .unwrap();
        let dep6_tx = TransactionView::new_advanced_builder()
            .cell_dep(node.always_success_cell_dep())
            .input(inputs.next().unwrap())
            .output(dep6_output)
            .output_data(dep6_data)
            .build();
        self.submit_transaction_until_committed(node, &dep5_tx);
        self.submit_transaction_until_committed(node, &dep6_tx);
        (dep5_tx, dep6_tx)
    }

    fn combine_cell_deps(
        &self,
        code_txs: (TransactionView, TransactionView),
        dep_group_txs: (TransactionView, TransactionView, TransactionView),
        incorrect_opt: Option<(TransactionView, TransactionView)>,
    ) -> CellDepSet {
        info!("[{}] >>> warm up: create all cell deps for test.", self.tag);
        let (dep1_tx, dep2_tx) = code_txs;
        let dep1_op = packed::OutPoint::new(dep1_tx.hash(), 0);
        let dep2_op = packed::OutPoint::new(dep2_tx.hash(), 0);
        let code1 = packed::CellDep::new_builder()
            .out_point(dep1_op)
            .dep_type(DepType::Code.into())
            .build();
        let code2 = packed::CellDep::new_builder()
            .out_point(dep2_op)
            .dep_type(DepType::Code.into())
            .build();
        let (dep3_tx, dep4_tx, dep4b_tx) = dep_group_txs;
        let dep_group1 = packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(dep3_tx.hash(), 0))
            .dep_type(DepType::DepGroup.into())
            .build();
        let dep_group2 = packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(dep4_tx.hash(), 0))
            .dep_type(DepType::DepGroup.into())
            .build();
        let dep_group2_copy = packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(dep4b_tx.hash(), 0))
            .dep_type(DepType::DepGroup.into())
            .build();
        let (code3, dep_group3) = if let Some((dep5_tx, dep6_tx)) = incorrect_opt {
            let dep3_op = packed::OutPoint::new(dep5_tx.hash(), 0);
            let code3 = packed::CellDep::new_builder()
                .out_point(dep3_op)
                .dep_type(DepType::Code.into())
                .build();
            let dep_group3 = packed::CellDep::new_builder()
                .out_point(packed::OutPoint::new(dep6_tx.hash(), 0))
                .dep_type(DepType::DepGroup.into())
                .build();
            (code3, dep_group3)
        } else {
            (Default::default(), Default::default())
        };
        CellDepSet {
            code1,
            code2,
            dep_group1,
            dep_group2,
            dep_group2_copy,
            code3,
            dep_group3,
        }
    }
}

// Implementation All Test Cases
impl DuplicateCellDepsTestRunner {
    fn test_result(
        &self,
        node: &Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_builder: TransactionBuilder,
        expected: ExpectedResult,
    ) {
        let empty_output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::shannons(0))
            .unwrap();
        let tx = tx_builder
            .input(inputs.next().unwrap())
            .output(empty_output)
            .output_data(Default::default())
            .build();
        if let Some(errmsg) = expected.error_message() {
            assert_send_transaction_fail(node, &tx, &errmsg);
        } else {
            self.submit_transaction_until_committed(node, &tx);
        }
    }

    fn test_single_code_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: duplicate code-type cell deps is {}.",
            self.tag, expected
        );
        let tx = tx_template.clone().cell_dep(deps.code1.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_single_dep_group_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: duplicate code-type cell deps is {}.",
            self.tag, expected
        );
        let tx = tx_template.clone().cell_dep(deps.dep_group1.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_duplicate_code_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: duplicate code-type cell deps is {}.",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.code1.clone())
            .cell_dep(deps.code1.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_data_code_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: two code-type cell deps have same data is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.code1.clone())
            .cell_dep(deps.code2.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_data_hybrid_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: hybrid-type cell deps have same data is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.code1.clone())
            .cell_dep(deps.dep_group1.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_duplicate_dep_group_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: duplicate dep_group-type cell deps is {}.",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.dep_group1.clone())
            .cell_dep(deps.dep_group1.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_data_dep_group_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: two dep_group-type cell deps have same data is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.dep_group1.clone())
            .cell_dep(deps.dep_group2.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_point_dep_group_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: two dep_group-type cell deps have a same point is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.dep_group2.clone())
            .cell_dep(deps.dep_group2_copy.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_type_not_same_data_code_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: two code-type cell deps have same type but not same data is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.code1.clone())
            .cell_dep(deps.code3.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_type_not_same_data_hybrid_type_v1(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: two hybrid-type cell deps have same type but not same data v1 is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.code1.clone())
            .cell_dep(deps.dep_group3.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_type_not_same_data_hybrid_type_v2(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: two hybrid-type cell deps have same type but not same data v2 is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.code3.clone())
            .cell_dep(deps.dep_group1.clone());
        self.test_result(node, inputs, tx, expected);
    }

    fn test_same_type_not_same_data_dep_group_type(
        &self,
        node: &Node,
        deps: &CellDepSet,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        tx_template: &TransactionBuilder,
        expected: ExpectedResult,
    ) {
        info!(
            "[{}] >>> test: two dep_group-type cell deps have same type but not same data is {}",
            self.tag, expected
        );
        let tx = tx_template
            .clone()
            .cell_dep(deps.dep_group1.clone())
            .cell_dep(deps.dep_group3.clone());
        self.test_result(node, inputs, tx, expected);
    }
}
