use crate::{
    util::{
        cell::gen_spendable,
        check::{assert_epoch_should_less_than, is_transaction_committed},
        mining::{mine, mine_until_bool, mine_until_epoch},
    },
    utils::assert_send_transaction_fail,
    Node, Spec,
};
use ckb_logger::{debug, info};
use ckb_types::{
    core::{Capacity, DepType, ScriptHashType, TransactionView},
    packed,
    prelude::*,
};
use std::fmt;

const GENESIS_EPOCH_LENGTH: u64 = 10;
const CKB2021_START_EPOCH: u64 = 10;

// ( Data, Type, Data1 ) * (Skip, Pass, Fail)
const TEST_CASES_COUNT: usize = 3 * 3;
const INITIAL_INPUTS_COUNT: usize = 1 + TEST_CASES_COUNT * 2;

pub struct CheckVmBExtension;

struct BExtScript {
    cell_dep: packed::CellDep,
    data_hash: packed::Byte32,
    type_hash: packed::Byte32,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedResult {
    ShouldBePassed,
    ValidationFailure,
    InvalidInstruction,
}

const PASS: ExpectedResult = ExpectedResult::ShouldBePassed;
const FAIL: ExpectedResult = ExpectedResult::ValidationFailure;
const INST: ExpectedResult = ExpectedResult::InvalidInstruction;

struct CheckVmBExtensionTestRunner<'a> {
    node: &'a Node,
    script: BExtScript,
}

impl Spec for CheckVmBExtension {
    crate::setup!(num_nodes: 1);

    fn run(&self, nodes: &mut Vec<Node>) {
        let epoch_length = GENESIS_EPOCH_LENGTH;
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;

        let node = &nodes[0];

        mine(node, 1);

        let mut inputs = gen_spendable(node, INITIAL_INPUTS_COUNT)
            .into_iter()
            .map(|input| packed::CellInput::new(input.out_point, 0));
        let script = BExtScript::new(node, inputs.next().unwrap());
        let runner = CheckVmBExtensionTestRunner::new(node, script);

        {
            info!("CKB v2019:");

            runner.do_test(&mut inputs, None, 0, 0, PASS);
            runner.do_test(&mut inputs, Some(0), 0, 0, PASS);

            runner.do_test(&mut inputs, None, 1, 0, INST);
            runner.do_test(&mut inputs, Some(0), 1, 0, INST);

            runner.do_test(&mut inputs, None, 1, 1, INST);
            runner.do_test(&mut inputs, Some(0), 1, 1, INST);
        }

        assert_epoch_should_less_than(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
        mine_until_epoch(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);

        {
            info!("CKB v2021:");

            runner.do_test(&mut inputs, None, 0, 0, PASS);
            runner.do_test(&mut inputs, Some(0), 0, 0, PASS);
            runner.do_test(&mut inputs, Some(1), 0, 0, PASS);

            runner.do_test(&mut inputs, None, 1, 0, FAIL);
            runner.do_test(&mut inputs, Some(0), 1, 0, INST);
            runner.do_test(&mut inputs, Some(1), 1, 0, FAIL);

            runner.do_test(&mut inputs, None, 1, 1, PASS);
            runner.do_test(&mut inputs, Some(0), 1, 1, INST);
            runner.do_test(&mut inputs, Some(1), 1, 1, PASS);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if spec.params.hardfork.is_none() {
            spec.params.hardfork = Some(Default::default());
        }
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_0032 = Some(CKB2021_START_EPOCH);
        }
    }
}

impl BExtScript {
    fn new(node: &Node, cell_input: packed::CellInput) -> Self {
        let data: packed::Bytes = include_bytes!("../../../../../script/testdata/cpop_lock").pack();
        let tx = Self::deploy(node, &data, cell_input);
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
            cell_dep,
            data_hash,
            type_hash,
        }
    }

    fn deploy(node: &Node, data: &packed::Bytes, cell_input: packed::CellInput) -> TransactionView {
        let type_script = node.always_success_script();
        let tx_template = TransactionView::new_advanced_builder();
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

    fn cell_dep(&self) -> packed::CellDep {
        self.cell_dep.clone()
    }

    fn as_data_script(&self, vm_version: u8, args: packed::Bytes) -> packed::Script {
        let hash_type = match vm_version {
            0 => ScriptHashType::Data,
            1 => ScriptHashType::Data1,
            _ => panic!("unknown vm_version [{}]", vm_version),
        };
        packed::Script::new_builder()
            .code_hash(self.data_hash.clone())
            .hash_type(hash_type.into())
            .args(args)
            .build()
    }

    fn as_type_script(&self, args: packed::Bytes) -> packed::Script {
        packed::Script::new_builder()
            .code_hash(self.type_hash.clone())
            .hash_type(ScriptHashType::Type.into())
            .args(args)
            .build()
    }

    fn as_script(&self, vm_version_opt: Option<u8>, args: packed::Bytes) -> packed::Script {
        if let Some(vm_version) = vm_version_opt {
            self.as_data_script(vm_version, args)
        } else {
            self.as_type_script(args)
        }
    }
}

impl fmt::Display for ExpectedResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ShouldBePassed => write!(f, "    allowed"),
            _ => write!(f, "not allowed"),
        }
    }
}

impl ExpectedResult {
    fn error_message(self) -> Option<&'static str> {
        match self {
            Self::ShouldBePassed => None,
            Self::ValidationFailure => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError { \
                 source: Outputs[0].Type, \
                 cause: ValidationFailure:",
            ),
            Self::InvalidInstruction => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError { \
                 source: Outputs[0].Type, \
                 cause: VM Internal Error: InvalidInstruction {",
            ),
        }
    }
}

impl<'a> CheckVmBExtensionTestRunner<'a> {
    fn new(node: &'a Node, script: BExtScript) -> Self {
        Self { node, script }
    }

    fn do_test(
        &self,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        vm_opt: Option<u8>,
        num0: u64,
        num1: u64,
        expected: ExpectedResult,
    ) {
        {
            let hash_type = match vm_opt {
                Some(0) => "Data",
                Some(1) => "Data1",
                None => "Type",
                _ => panic!("unknown vm_opt [{:?}]", vm_opt),
            };
            let inst = if num0 == 0 && num1 == 0 {
                "but skipped  "
            } else if num1 == u64::from(num0.count_ones()) {
                "and is passed"
            } else {
                "and is failed"
            };
            info!(
                ">>> test: {}-script has b-ext instructions {} is {}",
                hash_type, inst, expected
            );
        }
        let input = inputs.next().unwrap();
        let output = {
            let args: packed::Bytes = {
                let mut vec = Vec::with_capacity(8 * 2);
                vec.extend_from_slice(&num0.to_le_bytes());
                vec.extend_from_slice(&num1.to_le_bytes());
                vec.pack()
            };
            let script = self.script.as_script(vm_opt, args);
            packed::CellOutput::new_builder()
                .lock(self.node.always_success_script())
                .type_(Some(script).pack())
                .build_exact_capacity(Capacity::shannons(0))
                .unwrap()
        };
        let tx = TransactionView::new_advanced_builder()
            .cell_dep(self.node.always_success_cell_dep())
            .cell_dep(self.script.cell_dep())
            .input(input)
            .output(output)
            .output_data(Default::default())
            .build();
        if let Some(errmsg) = expected.error_message() {
            assert_send_transaction_fail(self.node, &tx, &errmsg);
        } else {
            self.submit_transaction_until_committed(&tx);
        }
    }

    fn submit_transaction_until_committed(&self, tx: &TransactionView) {
        debug!(">>> >>> submit: transaction {:#x}.", tx.hash());
        self.node.submit_transaction(tx);
        mine_until_bool(self.node, || is_transaction_committed(self.node, tx));
    }
}
