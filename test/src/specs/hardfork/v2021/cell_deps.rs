use crate::{
    util::{
        cell::gen_spendable,
        check::{
            assert_epoch_should_greater_than, assert_epoch_should_less_than,
            is_transaction_committed,
        },
    },
    utils::assert_send_transaction_fail,
    Node, Spec,
};
use ckb_jsonrpc_types as rpc;
use ckb_logger::{info, trace};
use ckb_types::packed::Byte32;
use ckb_types::{
    core::{self, TransactionView},
    packed,
    prelude::*,
};
use std::fmt;

const GENESIS_EPOCH_LENGTH: u64 = 10;
const CKB2021_START_EPOCH: u64 = 12;

// In `CheckCellDepsTestRunner::create_celldep_set()`:
// - Deploy 6 scripts.
// - Deploy 4 transactions as code cell deps.
// - Deploy 5 transactions as dep-group cell deps with 1 out point.
// - Deploy 4 transactions as dep-group cell deps with 2 out points.
// - Deploy 1 transactions as dep-group cell deps with 2048 out points.
// Each spends 1 input.
const CELL_DEPS_COST_COUNT: usize = 20;
// All test cases will truncate blocks after running, so we only require 1 transaction.
const TEST_CASES_COUNT: usize = 1;
const INITIAL_INPUTS_COUNT: usize = CELL_DEPS_COST_COUNT + TEST_CASES_COUNT;

pub struct CheckCellDeps;

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
    MultipleMatchesInputLock,
    MultipleMatchesInputType,
    MultipleMatchesOutputType,
    OverMaxDepExpansionLimitNotBan,
    OverMaxDepExpansionLimitBan,
}

// Use aliases to make the test cases matrix more readable.
type ER = ExpectedResult;
const PASS: ExpectedResult = ER::ShouldBePassed;
const DUP: ExpectedResult = ER::DuplicateCellDeps;
const MMIL: ExpectedResult = ER::MultipleMatchesInputLock;
const MMIT: ExpectedResult = ER::MultipleMatchesInputType;
const MMOT: ExpectedResult = ER::MultipleMatchesOutputType;
const MDEL_NOTBAN: ExpectedResult = ER::OverMaxDepExpansionLimitNotBan;
const MDEL_BAN: ExpectedResult = ER::OverMaxDepExpansionLimitBan;

// Use identifiers with same length to align the test cases matrix, to make it more readable.
#[derive(Debug, Clone, Copy)]
enum CellType {
    // The cell which requires those cell deps is an input.
    In,
    // The cell which requires those cell deps is an output.
    Ot,
    // No cell requires those cell deps.
    No,
}

#[derive(Debug, Clone, Copy)]
enum ScriptHashType {
    Data,
    Type,
}

#[derive(Debug, Clone, Copy)]
enum ScriptType {
    Lock,
    Type,
}

type CT = CellType;
type HT = ScriptHashType;
type ST = ScriptType;

//  Description:
//  - default_script:     a script has data a and type x.
//  - code_ax1:           cell dep: code, data: a, type x.
//  - code_ax2:           cell dep: code, data: a, type x.
//  - code_ay0:           cell dep: code, data: a, type y.
//  - code_bx0:           cell dep: code, data: b, type x.
//  - group_ax1a:         cell dep: dep group, point: [code_ax1].
//  - group_ax1b:         cell dep: dep group, point: [code_ax1].
//  - group_ax2:          cell dep: dep group, point: [code_ax2].
//  - group_ay0:          cell dep: dep group, point: [code_ay0].
//  - group_bx0:          cell dep: dep group, point: [code_bx0].
//  - group_ax1_ax1:      cell dep: dep group, point: [code_ax1, code_ax1].
//  - group_ax1_ax2:      cell dep: dep group, point: [code_ax1, code_ax2].
//  - group_ax1_ay0:      cell dep: dep group, point: [code_ax1, code_ay0].
//  - group_ax1_bx0:      cell dep: dep group, point: [code_ax1, code_bx0].
//  - group_ay0_2048:     cell dep: dep group, point: [code_ay0; 2048].
struct CellDepSet {
    default_script: NewScript,
    code_ax1: packed::CellDep,
    code_ax2: packed::CellDep,
    code_ay0: packed::CellDep,
    code_bx0: packed::CellDep,
    group_ax1a: packed::CellDep,
    group_ax1b: packed::CellDep,
    group_ax2: packed::CellDep,
    group_ay0: packed::CellDep,
    group_bx0: packed::CellDep,
    group_ax1_ax1: packed::CellDep,
    group_ax1_ax2: packed::CellDep,
    group_ax1_ay0: packed::CellDep,
    group_ax1_bx0: packed::CellDep,
    group_ay0_2048: packed::CellDep,
}

#[derive(Debug, Clone, Copy)]
enum RunnerState {
    V2019,
    OneBlockBeforeV2021,
    FirstBlockOfV2021,
    V2021,
}

struct CheckCellDepsTestRunner<'a> {
    node: &'a Node,
    deps: CellDepSet,
    inputs: Vec<packed::CellInput>,
    start_at: core::BlockNumber,
    checkpoint: core::BlockNumber,
    state: RunnerState,
}

impl Spec for CheckCellDeps {
    crate::setup!(num_nodes: 1);
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        let mut inputs = gen_spendable(node, INITIAL_INPUTS_COUNT)
            .into_iter()
            .map(|input| packed::CellInput::new(input.out_point, 0));
        let mut runner = CheckCellDepsTestRunner::new(node, &mut inputs);

        runner.start_at = node.get_tip_block_number();
        runner.switch_v2019();
        runner.run_v2019_tests();

        runner.switch_one_block_before_v2021();
        runner.run_v2019_tests();

        runner.switch_first_block_of_v2021();
        runner.run_v2021_tests();

        runner.switch_v2021();
        runner.run_v2021_tests();
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if spec.params.hardfork.is_none() {
            spec.params.hardfork = Some(Default::default());
        }
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_0029 = Some(CKB2021_START_EPOCH);
            switch.rfc_0038 = Some(CKB2021_START_EPOCH);
        }
    }
}

// Deploy a new always success script which has a different data hash with the default one.
//
// Append a byte to the default always success script.
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
            .dep_type(core::DepType::Code.into())
            .build();
        let data_hash = packed::CellOutput::calc_data_hash(&data.raw_data());
        let type_hash = tx
            .output(0)
            .unwrap()
            .type_()
            .to_opt()
            .unwrap()
            .calc_script_hash();
        trace!("NewScript({}) tx_hash    : {:#x}", id, tx.hash());
        trace!("NewScript({}) data_hash  : {:#x}", id, data_hash);
        trace!("NewScript({}) type_hash  : {:#x}", id, type_hash);
        let ret = Self {
            data,
            cell_dep,
            data_hash,
            type_hash,
        };
        let data_script_hash = ret.as_data_script(0).calc_script_hash();
        let type_script_hash = ret.as_type_script().calc_script_hash();
        trace!("NewScript({}) data_script: {:#x}", id, data_script_hash);
        trace!("NewScript({}) type_script: {:#x}", id, type_script_hash);
        ret
    }

    fn deploy(
        node: &Node,
        data: &packed::Bytes,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        type_script_opt: Option<&Self>,
    ) -> TransactionView {
        let (type_script, tx_template) = if let Some(script) = type_script_opt {
            (
                script.as_data_script(0),
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
            .build_exact_capacity(core::Capacity::bytes(data.len()).unwrap())
            .unwrap();
        let tx = tx_template
            .cell_dep(node.always_success_cell_dep())
            .input(cell_input)
            .output(cell_output)
            .output_data(data.clone())
            .build();
        node.submit_transaction(&tx);
        node.mine_until_bool(|| is_transaction_committed(node, &tx));
        tx
    }

    fn data(&self) -> packed::Bytes {
        self.data.clone()
    }

    fn cell_dep(&self) -> packed::CellDep {
        self.cell_dep.clone()
    }

    fn as_data_script(&self, vm_version: u8) -> packed::Script {
        let hash_type = match vm_version {
            0 => core::ScriptHashType::Data,
            1 => core::ScriptHashType::Data1,
            _ => panic!("unknown vm_version [{}]", vm_version),
        };
        packed::Script::new_builder()
            .code_hash(self.data_hash.clone())
            .hash_type(hash_type.into())
            .build()
    }

    fn as_type_script(&self) -> packed::Script {
        packed::Script::new_builder()
            .code_hash(self.type_hash.clone())
            .hash_type(core::ScriptHashType::Type.into())
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
            Self::MultipleMatchesInputLock => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError \
                 { source: Inputs[0].Lock, cause: MultipleMatches })",
            ),
            Self::MultipleMatchesInputType => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError \
                 { source: Inputs[0].Type, cause: MultipleMatches })",
            ),
            Self::MultipleMatchesOutputType => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError \
                 { source: Outputs[0].Type, cause: MultipleMatches })",
            ),
            Self::OverMaxDepExpansionLimitNotBan => Some(
                "{\"code\":-301,\"message\":\"TransactionFailedToResolve: \
                 Resolve failed OverMaxDepExpansionLimit\",\
                 \"data\":\"Resolve(OverMaxDepExpansionLimit { ban: false })\"}",
            ),
            Self::OverMaxDepExpansionLimitBan => Some(
                "{\"code\":-301,\"message\":\"TransactionFailedToResolve: \
                 Resolve failed OverMaxDepExpansionLimit\",\
                 \"data\":\"Resolve(OverMaxDepExpansionLimit { ban: true })\"}",
            ),
        }
    }
}

impl fmt::Display for CellType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::In => write!(f, "input"),
            Self::Ot => write!(f, "output"),
            Self::No => write!(f, "null"),
        }
    }
}

impl fmt::Display for ScriptHashType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Data => write!(f, "data"),
            Self::Type => write!(f, "type"),
        }
    }
}

impl fmt::Display for ScriptType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Lock => write!(f, "lock"),
            Self::Type => write!(f, "type"),
        }
    }
}

impl fmt::Display for RunnerState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::V2019 => write!(f, "v2019"),
            Self::OneBlockBeforeV2021 => write!(f, "v2021-"),
            Self::FirstBlockOfV2021 => write!(f, "v2021"),
            Self::V2021 => write!(f, "v2021+"),
        }
    }
}

impl<'a> CheckCellDepsTestRunner<'a> {
    fn new(node: &'a Node, inputs: &mut impl Iterator<Item = packed::CellInput>) -> Self {
        let deps = Self::create_celldep_set(node, inputs);
        let start_at = node.get_tip_block_number();
        let checkpoint = start_at;
        let inputs = inputs.collect();
        let state = RunnerState::V2019;
        Self {
            node,
            deps,
            inputs,
            start_at,
            checkpoint,
            state,
        }
    }

    fn submit_transaction_until_committed_to(node: &Node, tx: &TransactionView) {
        node.submit_transaction(tx);
        node.mine_until_transactions_confirm();
    }

    fn submit_transaction_until_committed(&self, tx: &TransactionView) {
        trace!(">> >>> submit: submit transaction {:#x}.", tx.hash());
        Self::submit_transaction_until_committed_to(self.node, tx)
    }

    fn restore_to_checkpoint(&self) {
        self.node.wait_for_tx_pool();
        let block_hash = self.node.get_block_by_number(self.checkpoint).hash();
        self.node.rpc_client().truncate(block_hash.clone());
        self.wait_block_assembler_reset(block_hash);
        self.node.wait_for_tx_pool();
    }

    fn switch_v2019(&mut self) {
        self.node.wait_for_tx_pool();
        let block_hash = self.node.get_block_by_number(self.start_at).hash();
        self.node.rpc_client().truncate(block_hash.clone());

        self.checkpoint = self.node.get_tip_block_number();
        self.state = RunnerState::V2019;
        self.wait_block_assembler_reset(block_hash);
        self.node.wait_for_tx_pool();
    }

    fn wait_block_assembler_reset(&self, block_hash: Byte32) {
        self.node
            .new_block_with_blocking(|template| template.parent_hash != block_hash.unpack());
    }

    fn switch_one_block_before_v2021(&mut self) {
        self.switch_v2019();

        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;
        let length = GENESIS_EPOCH_LENGTH;

        assert_epoch_should_less_than(self.node, ckb2019_last_epoch, 0, length);
        self.node.mine_until_epoch(ckb2019_last_epoch, 0, length);
        self.node.wait_for_tx_pool();

        self.checkpoint = self.node.get_tip_block_number();
        self.state = RunnerState::OneBlockBeforeV2021;
    }

    fn switch_first_block_of_v2021(&mut self) {
        self.switch_one_block_before_v2021();
        self.node.mine(1);
        self.node.wait_for_tx_pool();

        self.checkpoint = self.node.get_tip_block_number();
        self.state = RunnerState::FirstBlockOfV2021;
    }

    fn switch_v2021(&mut self) {
        self.switch_one_block_before_v2021();
        self.node.mine(1 + GENESIS_EPOCH_LENGTH * 2);
        self.node.wait_for_tx_pool();

        self.checkpoint = self.node.get_tip_block_number();
        self.state = RunnerState::V2021;
    }
}

// Create All Cell Deps for Test
impl<'a> CheckCellDepsTestRunner<'a> {
    fn create_celldep_set(
        node: &'a Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
    ) -> CellDepSet {
        let script0 = NewScript::new_with_id(node, 0, inputs, None);
        let data_a_script = NewScript::new_with_id(node, 1, inputs, Some(&script0));
        let data_b_script = NewScript::new_with_id(node, 2, inputs, Some(&script0));
        let type_x_script = NewScript::new_with_id(node, 3, inputs, Some(&script0));
        let type_y_script = NewScript::new_with_id(node, 4, inputs, Some(&script0));
        let default_script = NewScript::new_with_id(node, 1, inputs, Some(&type_x_script));
        let code_ax1_tx =
            Self::create_tx_as_code_celldep(node, inputs, &data_a_script, &type_x_script);
        let code_ax2_tx =
            Self::create_tx_as_code_celldep(node, inputs, &data_a_script, &type_x_script);
        let code_ay0_tx =
            Self::create_tx_as_code_celldep(node, inputs, &data_a_script, &type_y_script);
        let code_bx0_tx =
            Self::create_tx_as_code_celldep(node, inputs, &data_b_script, &type_x_script);
        let code_ax1 = Self::convert_tx_to_code_cellep(&code_ax1_tx);
        let code_ax2 = Self::convert_tx_to_code_cellep(&code_ax2_tx);
        let code_ay0 = Self::convert_tx_to_code_cellep(&code_ay0_tx);
        let code_bx0 = Self::convert_tx_to_code_cellep(&code_bx0_tx);
        let group_ax1a = Self::create_depgroup_celldep(node, inputs, &[&code_ax1_tx]);
        let group_ax1b = Self::create_depgroup_celldep(node, inputs, &[&code_ax1_tx]);
        let group_ax2 = Self::create_depgroup_celldep(node, inputs, &[&code_ax2_tx]);
        let group_ay0 = Self::create_depgroup_celldep(node, inputs, &[&code_ay0_tx]);
        let group_bx0 = Self::create_depgroup_celldep(node, inputs, &[&code_bx0_tx]);
        let group_ax1_ax1 =
            Self::create_depgroup_celldep(node, inputs, &[&code_ax1_tx, &code_ax1_tx]);
        let group_ax1_ax2 =
            Self::create_depgroup_celldep(node, inputs, &[&code_ax1_tx, &code_ax2_tx]);
        let group_ax1_ay0 =
            Self::create_depgroup_celldep(node, inputs, &[&code_ax1_tx, &code_ay0_tx]);
        let group_ax1_bx0 =
            Self::create_depgroup_celldep(node, inputs, &[&code_ax1_tx, &code_bx0_tx]);
        let group_ay0_2048 = Self::create_depgroup_celldep(node, inputs, &[&code_ay0_tx; 2048]);
        CellDepSet {
            default_script,
            code_ax1,
            code_ax2,
            code_ay0,
            code_bx0,
            group_ax1a,
            group_ax1b,
            group_ax2,
            group_ay0,
            group_bx0,
            group_ax1_ax1,
            group_ax1_ax2,
            group_ax1_ay0,
            group_ax1_bx0,
            group_ay0_2048,
        }
    }

    fn create_tx_as_code_celldep(
        node: &Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        data_script: &NewScript,
        type_script: &NewScript,
    ) -> TransactionView {
        let output = packed::CellOutput::new_builder()
            .type_(Some(type_script.as_data_script(0)).pack())
            .build_exact_capacity(core::Capacity::bytes(data_script.data().len()).unwrap())
            .unwrap();
        let tx = TransactionView::new_advanced_builder()
            .cell_dep(node.always_success_cell_dep())
            .cell_dep(type_script.cell_dep())
            .input(inputs.next().unwrap())
            .output(output)
            .output_data(data_script.data())
            .build();
        Self::submit_transaction_until_committed_to(node, &tx);
        tx
    }

    fn convert_tx_to_code_cellep(tx: &TransactionView) -> packed::CellDep {
        packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(tx.hash(), 0))
            .dep_type(core::DepType::Code.into())
            .build()
    }

    fn create_depgroup_celldep(
        node: &Node,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        dep_txs: &[&TransactionView],
    ) -> packed::CellDep {
        let dep_data = dep_txs
            .iter()
            .map(|tx| packed::OutPoint::new(tx.hash(), 0))
            .collect::<Vec<_>>()
            .pack()
            .as_bytes()
            .pack();
        let dep_output = packed::CellOutput::new_builder()
            .build_exact_capacity(core::Capacity::bytes(dep_data.len()).unwrap())
            .unwrap();
        let tx = TransactionView::new_advanced_builder()
            .cell_dep(node.always_success_cell_dep())
            .input(inputs.next().unwrap())
            .output(dep_output)
            .output_data(dep_data)
            .build();
        Self::submit_transaction_until_committed_to(node, &tx);
        packed::CellDep::new_builder()
            .out_point(packed::OutPoint::new(tx.hash(), 0))
            .dep_type(core::DepType::DepGroup.into())
            .build()
    }
}

// Create Cell Inputs for Test
impl<'a> CheckCellDepsTestRunner<'a> {
    fn create_initial_input(&self, tx: TransactionView) -> packed::CellInput {
        self.submit_transaction_until_committed(&tx);
        let out_point = packed::OutPoint::new(tx.hash(), 0);
        packed::CellInput::new(out_point, 0)
    }

    fn get_previous_output(&self, cell_input: &packed::CellInput) -> rpc::CellOutput {
        let previous_output = cell_input.previous_output();
        let previous_output_index: usize = previous_output.index().unpack();
        self.node
            .rpc_client()
            .get_transaction(previous_output.tx_hash())
            .unwrap()
            .transaction
            .unwrap()
            .inner
            .outputs[previous_output_index]
            .clone()
    }

    fn new_data_script_as_lock_script_input(
        &self,
        cell_input: packed::CellInput,
    ) -> packed::CellInput {
        let new_script = &self.deps.default_script;
        let input_cell = self.get_previous_output(&cell_input);
        let cell_output = packed::CellOutput::new_builder()
            .capacity(input_cell.capacity.value().pack())
            .lock(new_script.as_data_script(0))
            .build();
        let tx = TransactionView::new_advanced_builder()
            .cell_dep(self.node.always_success_cell_dep())
            .cell_dep(new_script.cell_dep())
            .input(cell_input)
            .output(cell_output)
            .output_data(Default::default())
            .build();
        self.create_initial_input(tx)
    }

    fn new_data_script_as_type_script_input(
        &self,
        cell_input: packed::CellInput,
    ) -> packed::CellInput {
        let new_script = &self.deps.default_script;
        let input_cell = self.get_previous_output(&cell_input);
        let cell_output = packed::CellOutput::new_builder()
            .capacity(input_cell.capacity.value().pack())
            .lock(self.node.always_success_script())
            .type_(Some(new_script.as_data_script(0)).pack())
            .build();
        let tx = TransactionView::new_advanced_builder()
            .cell_dep(self.node.always_success_cell_dep())
            .cell_dep(new_script.cell_dep())
            .input(cell_input)
            .output(cell_output)
            .output_data(Default::default())
            .build();
        self.create_initial_input(tx)
    }

    fn new_type_script_as_lock_script_input(
        &self,
        cell_input: packed::CellInput,
    ) -> packed::CellInput {
        let new_script = &self.deps.default_script;
        let input_cell = self.get_previous_output(&cell_input);
        let cell_output = packed::CellOutput::new_builder()
            .capacity(input_cell.capacity.value().pack())
            .lock(new_script.as_type_script())
            .build();
        let tx = TransactionView::new_advanced_builder()
            .cell_dep(self.node.always_success_cell_dep())
            .cell_dep(new_script.cell_dep())
            .input(cell_input)
            .output(cell_output)
            .output_data(Default::default())
            .build();
        self.create_initial_input(tx)
    }

    fn new_type_script_as_type_script_input(
        &self,
        cell_input: packed::CellInput,
    ) -> packed::CellInput {
        let new_script = &self.deps.default_script;
        let input_cell = self.get_previous_output(&cell_input);
        let cell_output = packed::CellOutput::new_builder()
            .capacity(input_cell.capacity.value().pack())
            .lock(self.node.always_success_script())
            .type_(Some(new_script.as_type_script()).pack())
            .build();
        let tx = TransactionView::new_advanced_builder()
            .cell_dep(self.node.always_success_cell_dep())
            .cell_dep(new_script.cell_dep())
            .input(cell_input)
            .output(cell_output)
            .output_data(Default::default())
            .build();
        self.create_initial_input(tx)
    }

    fn new_input(&self, ct: CellType, ht: ScriptHashType, st: ScriptType) -> packed::CellInput {
        let cell_input = self.inputs[0].clone();
        match ct {
            CT::In => match (ht, st) {
                (HT::Data, ST::Lock) => self.new_data_script_as_lock_script_input(cell_input),
                (HT::Data, ST::Type) => self.new_data_script_as_type_script_input(cell_input),
                (HT::Type, ST::Lock) => self.new_type_script_as_lock_script_input(cell_input),
                (HT::Type, ST::Type) => self.new_type_script_as_type_script_input(cell_input),
            },
            CT::Ot | CT::No => cell_input,
        }
    }
}

// Create Cell Outputs for Test
impl<'a> CheckCellDepsTestRunner<'a> {
    fn new_output(&self, ct: CellType, ht: ScriptHashType, st: ScriptType) -> packed::CellOutput {
        let cob = packed::CellOutput::new_builder();
        match ct {
            CT::In | CT::No => cob,
            CT::Ot => {
                let new_script = &self.deps.default_script;
                match (ht, st) {
                    (HT::Data, ST::Lock) => cob.lock(new_script.as_data_script(0)),
                    (HT::Data, ST::Type) => cob
                        .lock(self.node.always_success_script())
                        .type_(Some(new_script.as_data_script(0)).pack()),
                    (HT::Type, ST::Lock) => cob.lock(new_script.as_type_script()),
                    (HT::Type, ST::Type) => cob
                        .lock(self.node.always_success_script())
                        .type_(Some(new_script.as_type_script()).pack()),
                }
            }
        }
        .build_exact_capacity(core::Capacity::shannons(0))
        .unwrap()
    }
}

// Implementation All Test Cases
impl<'a> CheckCellDepsTestRunner<'a> {
    fn intro(
        &self,
        ct: CellType,
        ht: ScriptHashType,
        st: ScriptType,
        expected: ExpectedResult,
        casename: &str,
    ) {
        match ct {
            CT::No => {
                info!(
                    ">>> test {}/{}/----/----: {} should be {}.",
                    self.state, ct, casename, expected,
                );
            }
            _ => {
                info!(
                    ">>> test {}/{}/{}/{}: {} should be {}.",
                    self.state, ct, ht, st, casename, expected,
                );
            }
        }
    }

    fn adjust_tip_before_test(&self) {
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;
        let length = GENESIS_EPOCH_LENGTH;
        let blocks_to_commit_a_tx = self.node.consensus().tx_proposal_window().0 + 1;
        let index = length - blocks_to_commit_a_tx;
        match self.state {
            RunnerState::V2019 => {
                assert_epoch_should_less_than(self.node, ckb2019_last_epoch, index - 1, length);
            }
            RunnerState::OneBlockBeforeV2021 => {
                assert_epoch_should_less_than(self.node, ckb2019_last_epoch, index - 1, length);
                self.node
                    .mine_until_epoch(ckb2019_last_epoch, index - 1, length);
                self.node.wait_for_tx_pool();
            }
            RunnerState::FirstBlockOfV2021 => {
                assert_epoch_should_less_than(self.node, ckb2019_last_epoch, index - 1, length);
                self.node
                    .mine_until_epoch(ckb2019_last_epoch, index, length);
                self.node.wait_for_tx_pool();
            }
            RunnerState::V2021 => {
                assert_epoch_should_greater_than(self.node, CKB2021_START_EPOCH, 0, length);
            }
        }
    }

    fn test_result(
        &self,
        ct: CellType,
        ht: ScriptHashType,
        st: ScriptType,
        expected: ExpectedResult,
        deps: &[&packed::CellDep],
    ) {
        let cell_input = self.new_input(ct, ht, st);
        let cell_output = self.new_output(ct, ht, st);
        let tx = {
            let mut tb = TransactionView::new_advanced_builder();
            tb = match (ct, st) {
                (CT::In, ST::Type) | (CT::Ot, _) | (CT::No, _) => {
                    tb.cell_dep(self.node.always_success_cell_dep())
                }
                _ => tb,
            };
            for dep in deps {
                tb = tb.cell_dep((*dep).to_owned());
            }
            tb
        }
        .input(cell_input)
        .output(cell_output)
        .output_data(Default::default())
        .build();
        self.adjust_tip_before_test();
        if let Some(errmsg) = expected.error_message() {
            assert_send_transaction_fail(self.node, &tx, errmsg);
        } else {
            self.submit_transaction_until_committed(&tx);
        }
        self.restore_to_checkpoint();
    }

    fn run_v2019_tests(&self) {
        // Category: only one single code type cell dep
        self.test_single_code_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_single_code_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_single_code_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_single_code_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_single_code_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_single_code_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_single_code_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_single_code_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_single_code_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: only one dep group type cell dep
        self.test_single_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_single_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_single_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_single_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_single_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two duplicate code type cell deps
        self.test_duplicate_code_type(CT::In, HT::Data, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::In, HT::Data, ST::Type, DUP);
        self.test_duplicate_code_type(CT::In, HT::Type, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::In, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_code_type(CT::Ot, HT::Data, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::Ot, HT::Data, ST::Type, DUP);
        self.test_duplicate_code_type(CT::Ot, HT::Type, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::Ot, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_code_type(CT::No, HT::Data, ST::Lock, DUP);
        // Category: two duplicate dep group type cell deps
        self.test_duplicate_depgroup_type(CT::In, HT::Data, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::In, HT::Data, ST::Type, DUP);
        self.test_duplicate_depgroup_type(CT::In, HT::Type, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::In, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_depgroup_type(CT::Ot, HT::Data, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::Ot, HT::Data, ST::Type, DUP);
        self.test_duplicate_depgroup_type(CT::Ot, HT::Type, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::Ot, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_depgroup_type(CT::No, HT::Data, ST::Lock, DUP);
        // Category: two different code type cell deps have same data
        self.test_same_data_code_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_code_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_code_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_same_data_code_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_same_data_code_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_code_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_code_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_code_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_same_data_code_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two different dep group type cell deps have same out point
        self.test_same_outpoint_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_outpoint_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_outpoint_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_same_outpoint_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_same_outpoint_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two dep group type cell deps have different out points which have same data
        self.test_same_data_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_same_data_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_same_data_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_same_data_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have duplicate out points
        self.test_duplicate_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_duplicate_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_duplicate_in_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_duplicate_in_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_duplicate_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have different out points which have same data and
        // same type.
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_same_data_same_type_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have different out points which have same data and
        // different type.
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have same out point with a code type cell dep.
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_same_code_dep_for_hybrid_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have a dep which has same data and same type with
        // a code type cell dep.
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_same_data_same_type_for_hybrid_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have a dep which has same data but different type
        // with a code type cell dep, and the type in the code type cell dep is required.
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have a dep which has same data but different type
        // with a code type cell dep, and the type in the dep group type cell dep is required.
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two different code type cell deps have same type but different data
        self.test_diff_data_same_type_code_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_code_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_code_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_code_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_code_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two different code type cell deps have same data but different types
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have different out points which have same type but
        // different data.
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have same type with a code type cell dep but
        // different data, and the data in the code type cell dep is required.
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have same type with a code type cell dep but
        // different data, and the data in the dep group type cell dep is required.
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::No, HT::Data, ST::Lock, PASS);

        // Category: dep expansion count is 2048.
        self.test_dep_expansion_count_2048(PASS);
        // Category: dep expansion count is 2049.
        self.test_dep_expansion_count_2049(MDEL_NOTBAN);
    }

    fn run_v2021_tests(&self) {
        // Category: only one single code type cell dep
        self.test_single_code_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_single_code_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_single_code_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_single_code_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_single_code_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_single_code_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_single_code_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_single_code_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_single_code_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: only one dep group type cell dep
        self.test_single_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_single_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_single_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_single_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_single_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_single_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two duplicate code type cell deps
        self.test_duplicate_code_type(CT::In, HT::Data, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::In, HT::Data, ST::Type, DUP);
        self.test_duplicate_code_type(CT::In, HT::Type, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::In, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_code_type(CT::Ot, HT::Data, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::Ot, HT::Data, ST::Type, DUP);
        self.test_duplicate_code_type(CT::Ot, HT::Type, ST::Lock, DUP);
        self.test_duplicate_code_type(CT::Ot, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_code_type(CT::No, HT::Data, ST::Lock, DUP);
        // Category: two duplicate dep group type cell deps
        self.test_duplicate_depgroup_type(CT::In, HT::Data, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::In, HT::Data, ST::Type, DUP);
        self.test_duplicate_depgroup_type(CT::In, HT::Type, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::In, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_depgroup_type(CT::Ot, HT::Data, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::Ot, HT::Data, ST::Type, DUP);
        self.test_duplicate_depgroup_type(CT::Ot, HT::Type, ST::Lock, DUP);
        self.test_duplicate_depgroup_type(CT::Ot, HT::Type, ST::Type, DUP);
        //
        self.test_duplicate_depgroup_type(CT::No, HT::Data, ST::Lock, DUP);
        // Category: two different code type cell deps have same data
        self.test_same_data_code_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_code_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_code_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_code_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_code_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_code_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_code_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_code_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_code_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two different dep group type cell deps have same out point
        self.test_same_outpoint_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_outpoint_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_outpoint_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_outpoint_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_outpoint_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_outpoint_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two dep group type cell deps have different out points which have same data
        self.test_same_data_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have duplicate out points
        self.test_duplicate_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_duplicate_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_duplicate_in_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_duplicate_in_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_duplicate_in_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_duplicate_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have different out points which have same data and
        // same type.
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_same_type_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have different out points which have same data and
        // different type.
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_in_depgroup_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have same out point with a code type cell dep.
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_code_dep_for_hybrid_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_code_dep_for_hybrid_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have a dep which has same data and same type with
        // a code type cell dep.
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_same_type_for_hybrid_type(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_same_type_for_hybrid_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have a dep which has same data but different type
        // with a code type cell dep, and the type in the code type cell dep is required.
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v1(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have a dep which has same data but different type
        // with a code type cell dep, and the type in the dep group type cell dep is required.
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Type, PASS);
        //
        self.test_same_data_diff_type_for_hybrid_type_v2(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two different code type cell deps have same type but different data
        self.test_diff_data_same_type_code_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_code_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_code_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_code_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_code_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_code_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: two different code type cell deps have same data but different types
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have different out points which have same type but
        // different data.
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_in_depgroup_type(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_in_depgroup_type(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_in_depgroup_type(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have same type with a code type cell dep but
        // different data, and the data in the code type cell dep is required.
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v1(CT::No, HT::Data, ST::Lock, PASS);
        // Category: a dep group type cell dep have same type with a code type cell dep but
        // different data, and the data in the dep group type cell dep is required.
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Lock, MMIL);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::In, HT::Type, ST::Type, MMIT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Data, ST::Type, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Lock, PASS);
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::Ot, HT::Type, ST::Type, MMOT);
        //
        self.test_diff_data_same_type_for_hybrid_type_v2(CT::No, HT::Data, ST::Lock, PASS);

        // Category: dep expansion count is 2048.
        self.test_dep_expansion_count_2048(PASS);
        // Category: dep expansion count is 2049.
        self.test_dep_expansion_count_2049(MDEL_BAN);
    }

    fn test_single_code_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "single code-type dep");
        let deps = &[&self.deps.code_ax1];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_single_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "single dep_group-type dep");
        let deps = &[&self.deps.group_ax1a];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_duplicate_code_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "duplicate code-type deps");
        let deps = &[&self.deps.code_ax1, &self.deps.code_ax1];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_duplicate_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "duplicate dep_group-type deps");
        let deps = &[&self.deps.group_ax1a, &self.deps.group_ax1a];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_data_code_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same data same type code-type deps");
        let deps = &[&self.deps.code_ax1, &self.deps.code_ax2];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_outpoint_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same outpoint dep_group-type deps");
        let deps = &[&self.deps.group_ax1a, &self.deps.group_ax1b];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_data_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same data same type dep_group-type deps");
        let deps = &[&self.deps.group_ax1a, &self.deps.group_ax2];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_duplicate_in_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "duplicate in dep_group-type dep");
        let deps = &[&self.deps.group_ax1_ax1];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_data_same_type_in_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same data same type in dep_group-type dep");
        let deps = &[&self.deps.group_ax1_ax2];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_data_diff_type_in_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same data diff type in dep_group-type dep");
        let deps = &[&self.deps.group_ax1_ay0];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_code_dep_for_hybrid_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same code dep for hybrid-type deps");
        let deps = &[&self.deps.code_ax1, &self.deps.group_ax1a];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_data_same_type_for_hybrid_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same data same type for hybrid-type deps");
        let deps = &[&self.deps.code_ax2, &self.deps.group_ax1a];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_data_diff_type_for_hybrid_type_v1(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same data diff type for hybrid-type deps 1");
        let deps = &[&self.deps.code_ax1, &self.deps.group_ay0];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_same_data_diff_type_for_hybrid_type_v2(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "same data diff type for hybrid-type deps 2");
        let deps = &[&self.deps.code_ay0, &self.deps.group_ax1a];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_diff_data_same_type_code_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "diff data same type code-type deps");
        let deps = &[&self.deps.code_ax1, &self.deps.code_bx0];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_diff_data_same_type_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "diff data same type depgroup-type deps");
        let deps = &[&self.deps.group_ax1a, &self.deps.group_bx0];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_diff_data_same_type_in_depgroup_type(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "diff data same type in dep_group-type dep");
        let deps = &[&self.deps.group_ax1_bx0];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_diff_data_same_type_for_hybrid_type_v1(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "diff data same type for hybrid-type deps 1");
        let deps = &[&self.deps.code_ax1, &self.deps.group_bx0];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_diff_data_same_type_for_hybrid_type_v2(&self, ct: CT, ht: HT, st: ST, er: ER) {
        self.intro(ct, ht, st, er, "diff data same type for hybrid-type deps 2");
        let deps = &[&self.deps.code_bx0, &self.deps.group_ax1a];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_dep_expansion_count_2048(&self, er: ER) {
        let (ct, ht, st) = (CT::In, HT::Data, ST::Lock);
        self.intro(ct, ht, st, er, "dep expansion count is 2048");
        let deps = &[&self.deps.group_ay0_2048];
        self.test_result(ct, ht, st, er, deps);
    }

    fn test_dep_expansion_count_2049(&self, er: ER) {
        let (ct, ht, st) = (CT::In, HT::Data, ST::Lock);
        self.intro(ct, ht, st, er, "dep expansion count is 2049");
        let deps = &[&self.deps.code_bx0, &self.deps.group_ay0_2048];
        self.test_result(ct, ht, st, er, deps);
    }
}
