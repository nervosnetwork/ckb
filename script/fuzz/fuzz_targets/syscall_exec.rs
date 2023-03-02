#![no_main]
use arbitrary::Arbitrary;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_script::{TransactionScriptsVerifier, TxVerifyEnv};
use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{CellMetaBuilder, ResolvedTransaction},
        Capacity, HeaderView, ScriptHashType, TransactionBuilder, TransactionInfo,
    },
    packed::{
        Byte32, CellInput, CellOutput, CellOutputBuilder, OutPoint, Script, TransactionInfoBuilder,
        TransactionKeyBuilder,
    },
    prelude::*,
};
use libfuzzer_sys::fuzz_target;

#[derive(Default, PartialEq, Eq, Clone)]
struct MockDataLoader {}

impl CellDataProvider for MockDataLoader {
    fn get_cell_data(&self, _out_point: &OutPoint) -> Option<Bytes> {
        None
    }

    fn get_cell_data_hash(&self, _out_point: &OutPoint) -> Option<Byte32> {
        None
    }
}

impl HeaderProvider for MockDataLoader {
    fn get_header(&self, _block_hash: &Byte32) -> Option<HeaderView> {
        None
    }
}

fn mock_transaction_info() -> TransactionInfo {
    TransactionInfoBuilder::default()
        .block_number(1u64.pack())
        .block_epoch(0u64.pack())
        .key(
            TransactionKeyBuilder::default()
                .block_hash(Byte32::zero())
                .index(1u32.pack())
                .build(),
        )
        .build()
        .unpack()
}

static CALLER: &[u8] = include_bytes!("../programs/exec_caller");
static CALLEE: &[u8] = include_bytes!("../programs/exec_callee");

#[derive(Arbitrary, Debug)]
pub struct FuzzData {
    from: u32,
    argv: Vec<String>,
    callee_data_head: u64,
    callee_data_tail: u64,
}

fn run(data: FuzzData) {
    let exec_caller_cell_data = Bytes::from(CALLER);
    let exec_callee_cell_data = {
        let mut r: Vec<u8> = vec![];
        for _ in 0..data.callee_data_head as u8 {
            r.push(0x00);
        }
        r.extend(CALLEE);
        for _ in 0..data.callee_data_tail as u8 {
            r.push(0x00);
        }
        Bytes::copy_from_slice(&r)
    };
    let exec_caller_data_data = {
        let mut r: Vec<u8> = vec![];
        r.push(data.from as u8 % 3);
        r.push(data.callee_data_head as u8);
        let l = if data.callee_data_tail as u8 == 0 {
            0
        } else {
            CALLEE.len()
        };
        r.extend_from_slice(&l.to_le_bytes());
        let argc = data.argv.len() as u64;
        r.extend_from_slice(&argc.to_le_bytes());
        for i in &data.argv {
            let l = i.len() as u64 + 1;
            r.extend_from_slice(&l.to_le_bytes());
            r.extend_from_slice(i.as_bytes());
            r.push(0x00);
        }
        Bytes::copy_from_slice(&r)
    };

    let exec_caller_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_caller_cell_data.len()).unwrap().pack())
        .build();
    let exec_callee_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_callee_cell_data.len()).unwrap().pack())
        .build();
    let exec_caller_data_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_caller_data_data.len()).unwrap().pack())
        .build();

    let exec_caller_script = Script::new_builder()
        .hash_type(ScriptHashType::Data1.into())
        .code_hash(CellOutput::calc_data_hash(&exec_caller_cell_data))
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .set_witnesses(vec![exec_callee_cell_data.pack()])
        .build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(mock_transaction_info())
        .build();
    let exec_caller_cell =
        CellMetaBuilder::from_cell_output(exec_caller_cell, exec_caller_cell_data)
            .transaction_info(mock_transaction_info())
            .build();

    let exec_callee_cell =
        CellMetaBuilder::from_cell_output(exec_callee_cell, exec_callee_cell_data)
            .transaction_info(mock_transaction_info())
            .build();
    let exec_caller_data_cell =
        CellMetaBuilder::from_cell_output(exec_caller_data_cell, exec_caller_data_data)
            .transaction_info(mock_transaction_info())
            .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![exec_caller_cell, exec_callee_cell, exec_caller_data_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let proivder = MockDataLoader {};
    let verifier = TransactionScriptsVerifier::new(&rtx, &proivder);

    let result = verifier.verify(10_000_000_000);
    assert!(result.is_ok());
}

fuzz_target!(|data: FuzzData| {
    run(data);
});
