use crate::{
    util::{
        cell::gen_spendable,
        check::{assert_epoch_should_less_than, is_transaction_committed},
    },
    utils::{assert_send_transaction_fail, wait_until},
    Node, Spec,
};
use ckb_jsonrpc_types as rpc;
use ckb_logger::{debug, info};
use ckb_types::{
    core::{Capacity, DepType, ScriptHashType, TransactionView},
    packed,
    prelude::*,
};
use std::fmt;

const RPC_MAX_VM_VERSION: u8 = 1;
const MAX_VM_VERSION: u8 = 1;

const GENESIS_EPOCH_LENGTH: u64 = 10;
const CKB2021_START_EPOCH: u64 = 10;

const TEST_CASES_COUNT: usize = (RPC_MAX_VM_VERSION as usize + 1 + 1) * 2;
const INITIAL_INPUTS_COUNT: usize = 1 + TEST_CASES_COUNT * 2;

pub struct CheckVmVersion;

struct NewScript {
    cell_dep: packed::CellDep,
    data_hash: packed::Byte32,
    type_hash: packed::Byte32,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedResult {
    ShouldBePassed,
    IncompatibleVmV1,
    RpcInvalidVmVersion,
    LockInvalidVmVersion,
    TypeInvalidVmVersion,
}

struct CheckVmVersionTestRunner<'a> {
    node: &'a Node,
}

impl Spec for CheckVmVersion {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let epoch_length = GENESIS_EPOCH_LENGTH;
        let ckb2019_last_epoch = CKB2021_START_EPOCH - 1;

        let node = &nodes[0];
        let node1 = &nodes[1];

        node.mine(1);
        node1.connect(node);

        {
            let mut inputs = gen_spendable(node, INITIAL_INPUTS_COUNT)
                .into_iter()
                .map(|input| packed::CellInput::new(input.out_point, 0));
            let script = NewScript::new_with_id(node, 0, &mut inputs);
            let runner = CheckVmVersionTestRunner::new(node);

            info!("CKB v2019:");
            runner.run_all_tests(&mut inputs, &script, 0);

            assert_epoch_should_less_than(node, ckb2019_last_epoch, epoch_length - 4, epoch_length);
            node.mine_until_epoch(ckb2019_last_epoch, epoch_length - 4, epoch_length);

            info!("CKB v2021:");
            runner.run_all_tests(&mut inputs, &script, 1);
        }

        {
            info!("Test Sync:");
            let (rpc_client0, rpc_client1) = (node.rpc_client(), node1.rpc_client());

            // The GetHeaders will be sent every 15s.
            // When reach tip, the GetHeaders will be paused 28s.
            let ret = wait_until(60, || {
                let header0 = rpc_client0.get_tip_header();
                let header1 = rpc_client1.get_tip_header();
                header0 == header1
            });
            assert!(
                ret,
                "Nodes should sync with each other until same tip chain",
            );
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

impl NewScript {
    fn new_with_id(
        node: &Node,
        id: u8,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
    ) -> Self {
        let original_data = node.always_success_raw_data();
        let data = packed::Bytes::new_builder()
            .extend(original_data.as_ref().iter().map(|x| (*x).into()))
            .push(id.into())
            .build();
        let tx = Self::deploy(node, &data, inputs);
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

    fn deploy(
        node: &Node,
        data: &packed::Bytes,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
    ) -> TransactionView {
        let type_script = node.always_success_script();
        let tx_template = TransactionView::new_advanced_builder();
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
        node.mine_until_bool(|| is_transaction_committed(node, &tx));
        tx
    }

    fn cell_dep(&self) -> packed::CellDep {
        self.cell_dep.clone()
    }

    fn as_data_script(&self, vm_version: u8) -> packed::Script {
        let hash_type = match vm_version {
            0 => ScriptHashType::Data,
            1 => ScriptHashType::Data1,
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
            .hash_type(ScriptHashType::Type.into())
            .build()
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
            Self::IncompatibleVmV1 => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Transaction(Compatible: \
                 the feature \\\"VM Version 1\\\"",
            ),
            Self::RpcInvalidVmVersion => Some(
                "{\"code\":-32602,\"message\":\"\
                 Invalid params: the maximum vm version currently supported is",
            ),
            Self::LockInvalidVmVersion => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError \
                 { source: Inputs[0].Lock, cause: Invalid VM Version:",
            ),
            Self::TypeInvalidVmVersion => Some(
                "{\"code\":-302,\"message\":\"TransactionFailedToVerify: \
                 Verification failed Script(TransactionScriptError { \
                 source: Outputs[0].Type, cause: Invalid VM Version: ",
            ),
        }
    }
}

impl<'a> CheckVmVersionTestRunner<'a> {
    fn new(node: &'a Node) -> Self {
        Self { node }
    }

    fn test_create(
        &self,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        cell_dep_opt: Option<packed::CellDep>,
        script: packed::Script,
        expected: ExpectedResult,
    ) -> Option<TransactionView> {
        let (tx_builder, co_builder) = if let Some(cell_dep) = cell_dep_opt {
            (
                TransactionView::new_advanced_builder().cell_dep(cell_dep),
                packed::CellOutput::new_builder()
                    .lock(self.node.always_success_script())
                    .type_(Some(script).pack()),
            )
        } else {
            (
                TransactionView::new_advanced_builder(),
                packed::CellOutput::new_builder().lock(script),
            )
        };
        let cell_input = inputs.next().unwrap();
        let input_cell = self.get_previous_output(&cell_input);
        let cell_output = co_builder
            .capacity((input_cell.capacity.value() - 1).pack())
            .build();
        let tx = tx_builder
            .cell_dep(self.node.always_success_cell_dep())
            .input(cell_input)
            .output(cell_output)
            .output_data(Default::default())
            .build();
        if let Some(errmsg) = expected.error_message() {
            assert_send_transaction_fail(self.node, &tx, errmsg);
            None
        } else {
            self.submit_transaction_until_committed(&tx);
            Some(tx)
        }
    }

    fn test_spend(
        &self,
        tx: TransactionView,
        cell_dep: packed::CellDep,
        has_always_success: bool,
        expected: ExpectedResult,
    ) {
        let out_point = packed::OutPoint::new(tx.hash(), 0);
        let input = packed::CellInput::new(out_point, 0);
        let output = packed::CellOutput::new_builder()
            .build_exact_capacity(Capacity::shannons(0))
            .unwrap();
        let tx = if has_always_success {
            TransactionView::new_advanced_builder().cell_dep(self.node.always_success_cell_dep())
        } else {
            TransactionView::new_advanced_builder()
        }
        .cell_dep(cell_dep)
        .input(input)
        .output(output)
        .output_data(Default::default())
        .build();
        if let Some(errmsg) = expected.error_message() {
            assert_send_transaction_fail(self.node, &tx, errmsg);
        } else {
            self.submit_transaction_until_committed(&tx);
        }
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

    fn submit_transaction_until_committed(&self, tx: &TransactionView) {
        debug!(">>> >>> submit: transaction {:#x}.", tx.hash());
        self.node.submit_transaction(tx);
        self.node
            .mine_until_bool(|| is_transaction_committed(self.node, tx));
    }

    fn run_all_tests(
        &self,
        inputs: &mut impl Iterator<Item = packed::CellInput>,
        script: &NewScript,
        max_vm_version: u8,
    ) {
        for vm_version in 0..=RPC_MAX_VM_VERSION {
            let res = if vm_version <= max_vm_version {
                ExpectedResult::ShouldBePassed
            } else if vm_version <= MAX_VM_VERSION {
                ExpectedResult::IncompatibleVmV1
            } else {
                ExpectedResult::RpcInvalidVmVersion
            };
            info!(
                ">>> Create a   cell with Data({:2}) lock script is {}",
                vm_version, res
            );
            let s = script.as_data_script(vm_version);
            if let Some(tx) = self.test_create(inputs, None, s, res) {
                let res = if vm_version <= max_vm_version {
                    ExpectedResult::ShouldBePassed
                } else {
                    ExpectedResult::LockInvalidVmVersion
                };
                info!(
                    ">>> Spend the  cell with Data({:2}) lock script is {}",
                    vm_version, res
                );
                let dep = script.cell_dep();
                self.test_spend(tx, dep, false, res);
            }
        }
        {
            let res = ExpectedResult::ShouldBePassed;
            info!(">>> Create a   cell with Type     lock script is {}", res);
            let s = script.as_type_script();
            if let Some(tx) = self.test_create(inputs, None, s, res) {
                let res = ExpectedResult::ShouldBePassed;
                info!(">>> Spend the  cell with Type     lock script is {}", res);
                let dep = script.cell_dep();
                self.test_spend(tx, dep, false, res);
            }
        }
        for vm_version in 0..=RPC_MAX_VM_VERSION {
            let res = if vm_version <= max_vm_version {
                ExpectedResult::ShouldBePassed
            } else if vm_version <= MAX_VM_VERSION {
                ExpectedResult::TypeInvalidVmVersion
            } else {
                ExpectedResult::RpcInvalidVmVersion
            };
            info!(
                ">>> Create a   cell with Data({:2}) type script is {}",
                vm_version, res
            );
            let dep = Some(script.cell_dep());
            let s = script.as_data_script(vm_version);
            if let Some(tx) = self.test_create(inputs, dep, s, res) {
                let res = if vm_version <= max_vm_version {
                    ExpectedResult::ShouldBePassed
                } else {
                    ExpectedResult::TypeInvalidVmVersion
                };
                info!(
                    ">>> Spend the  cell with Data({:2}) type script is {}",
                    vm_version, res
                );
                let dep = script.cell_dep();
                self.test_spend(tx, dep, true, res);
            }
        }
        {
            let res = ExpectedResult::ShouldBePassed;
            info!(">>> Create a   cell with Type     type script is {}", res);
            let dep = Some(script.cell_dep());
            let s = script.as_type_script();
            if let Some(tx) = self.test_create(inputs, dep, s, res) {
                let res = ExpectedResult::ShouldBePassed;
                info!(">>> Spend the  cell with Type     type script is {}", res);
                let dep = script.cell_dep();
                self.test_spend(tx, dep, true, res);
            }
        }
    }
}
