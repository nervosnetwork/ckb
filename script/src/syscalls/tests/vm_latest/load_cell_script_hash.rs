use ckb_types::{
    bytes::Bytes,
    core::{
        ScriptHashType, TransactionBuilder,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{Byte32, CellOutput, Script},
    prelude::*,
};
use ckb_vm::{
    CoreMachine, Memory, SupportMachine, Syscalls,
    registers::{A0, A1, A2, A3, A4, A5, A7},
};
use std::sync::Arc;

use super::SCRIPT_VERSION;
use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{tests::utils::*, *},
    types::{CellScriptHashes, SgData},
};

// Large enough that hashing it dominates a syscall, small enough for unit tests.
const LARGE_ARGS_SIZE: usize = 64 * 1024;

fn large_script(tag: u8) -> Script {
    Script::new_builder()
        .args(Bytes::from(vec![tag; LARGE_ARGS_SIZE]))
        .hash_type(ScriptHashType::Data)
        .build()
}

fn cell_with_scripts(lock: Script, type_: Option<Script>) -> CellMeta {
    let mut cell = build_cell_meta(1000, Bytes::new());
    cell.cell_output = cell
        .cell_output
        .as_builder()
        .lock(lock)
        .type_(type_)
        .build();
    cell
}

/// Transaction with 2 inputs, 2 outputs and 2 cell deps. The cell at index 0
/// of each source has both lock and type scripts, the cell at index 1 only
/// has a lock script.
fn build_rtx() -> Arc<ResolvedTransaction> {
    let outputs: Vec<CellOutput> = vec![
        CellOutput::new_builder()
            .lock(large_script(5))
            .type_(Some(large_script(6)))
            .build(),
        CellOutput::new_builder().lock(large_script(7)).build(),
    ];
    let transaction = TransactionBuilder::default()
        .outputs(outputs)
        .outputs_data(vec![Bytes::new().pack(), Bytes::new().pack()])
        .build();
    Arc::new(ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![
            cell_with_scripts(large_script(3), Some(large_script(4))),
            cell_with_scripts(large_script(8), None),
        ],
        resolved_inputs: vec![
            cell_with_scripts(large_script(1), Some(large_script(2))),
            cell_with_scripts(large_script(9), None),
        ],
        resolved_dep_groups: vec![],
    })
}

/// SgData built by `TxData::new`, which populates the script hash cache.
fn build_cached_sg_data(rtx: Arc<ResolvedTransaction>) -> SgData<MockDataLoader> {
    build_sg_data_with_derived_tx_info(rtx, vec![0, 1], vec![0, 1])
}

/// SgData without any cached script hashes, i.e. the behavior before memoization.
fn build_uncached_sg_data(rtx: Arc<ResolvedTransaction>) -> SgData<MockDataLoader> {
    let outputs = build_cached_sg_data(Arc::clone(&rtx))
        .tx_info
        .outputs
        .clone();
    let sg_data = build_sg_data(rtx, vec![0, 1], vec![0, 1]);
    update_tx_info(sg_data, |tx_info| {
        tx_info.outputs = outputs.clone();
        assert!(tx_info.script_hash_cache.inputs.is_empty());
    })
}

#[derive(Debug, PartialEq, Eq)]
struct SyscallResult {
    return_code: u64,
    full_size: u64,
    data: Vec<u8>,
    cycles: u64,
}

fn load_cell_by_field(
    sg_data: &SgData<MockDataLoader>,
    source: Source,
    index: u64,
    field: CellField,
    buffer_size: u64,
) -> SyscallResult {
    let mut machine = SCRIPT_VERSION.init_core_machine_without_limit();
    let size_addr: u64 = 0;
    let addr: u64 = 100;

    machine.set_register(A0, addr);
    machine.set_register(A1, size_addr);
    machine.set_register(A2, 0);
    machine.set_register(A3, index);
    machine.set_register(A4, u64::from(source));
    machine.set_register(A5, field as u64);
    machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER);
    machine
        .memory_mut()
        .store64(&size_addr, &buffer_size)
        .unwrap();

    let mut load_cell = LoadCell::new(sg_data);
    assert!(load_cell.ecall(&mut machine).unwrap());

    let return_code = machine.registers()[A0];
    let full_size = machine.memory_mut().load64(&size_addr).unwrap();
    let written = if return_code == u64::from(SUCCESS) {
        full_size.min(buffer_size)
    } else {
        0
    };
    let data = (addr..addr + written)
        .map(|a| machine.memory_mut().load8(&a).unwrap() as u8)
        .collect();
    SyscallResult {
        return_code,
        full_size,
        data,
        cycles: machine.cycles(),
    }
}

const ALL_SOURCES: [(Source, u64); 10] = [
    (Source::Transaction(SourceEntry::Input), 0),
    (Source::Transaction(SourceEntry::Input), 1),
    (Source::Transaction(SourceEntry::Output), 0),
    (Source::Transaction(SourceEntry::Output), 1),
    (Source::Transaction(SourceEntry::CellDep), 0),
    (Source::Transaction(SourceEntry::CellDep), 1),
    (Source::Group(SourceEntry::Input), 0),
    (Source::Group(SourceEntry::Input), 1),
    (Source::Group(SourceEntry::Output), 0),
    (Source::Group(SourceEntry::Output), 1),
];

fn expected_hash(
    rtx: &ResolvedTransaction,
    source: Source,
    index: usize,
    field: CellField,
) -> Option<Byte32> {
    let output = match source {
        Source::Transaction(SourceEntry::Input) | Source::Group(SourceEntry::Input) => {
            rtx.resolved_inputs[index].cell_output.clone()
        }
        Source::Transaction(SourceEntry::Output) | Source::Group(SourceEntry::Output) => {
            rtx.transaction.outputs().get(index).unwrap()
        }
        Source::Transaction(SourceEntry::CellDep) => {
            rtx.resolved_cell_deps[index].cell_output.clone()
        }
        _ => unreachable!(),
    };
    match field {
        CellField::LockHash => Some(output.calc_lock_hash()),
        CellField::TypeHash => output.type_().to_opt().map(|t| t.calc_script_hash()),
        _ => unreachable!(),
    }
}

/// Memoization must not change anything observable by scripts: return code,
/// returned data, reported size and, most importantly, consumed cycles,
/// which are part of consensus.
#[test]
fn test_load_cell_script_hash_cache_keeps_results_and_cycles() {
    let rtx = build_rtx();
    let cached = build_cached_sg_data(Arc::clone(&rtx));
    let uncached = build_uncached_sg_data(Arc::clone(&rtx));

    for (source, index) in ALL_SOURCES {
        for field in [CellField::LockHash, CellField::TypeHash] {
            let expected = expected_hash(&rtx, source, index as usize, field);
            for buffer_size in [0u64, 1, 16, 32, 64] {
                // Call twice so the second call hits the cache.
                let first = load_cell_by_field(&cached, source, index, field, buffer_size);
                let second = load_cell_by_field(&cached, source, index, field, buffer_size);
                let baseline = load_cell_by_field(&uncached, source, index, field, buffer_size);

                assert_eq!(
                    first, baseline,
                    "{source:?} {index} {field:?} {buffer_size}"
                );
                assert_eq!(
                    second, baseline,
                    "{source:?} {index} {field:?} {buffer_size}"
                );

                match &expected {
                    Some(hash) => {
                        let written = buffer_size.min(32);
                        assert_eq!(baseline.return_code, u64::from(SUCCESS));
                        assert_eq!(baseline.full_size, 32);
                        assert_eq!(baseline.data, hash.as_slice()[..written as usize]);
                        assert_eq!(baseline.cycles, transferred_byte_cycles(written));
                    }
                    None => {
                        assert_eq!(baseline.return_code, u64::from(ITEM_MISSING));
                        assert_eq!(baseline.cycles, 0);
                    }
                }
            }
        }
    }
}

#[test]
fn test_tx_data_seeds_script_hash_cache() {
    let rtx = build_rtx();
    let sg_data = build_cached_sg_data(Arc::clone(&rtx));
    let cache = &sg_data.tx_info.script_hash_cache;

    assert_eq!(cache.inputs.len(), rtx.resolved_inputs.len());
    assert_eq!(cache.outputs.len(), rtx.transaction.outputs().len());
    assert_eq!(cache.cell_deps.len(), rtx.resolved_cell_deps.len());

    // Hashes already calculated to build script groups and binaries index
    // are reused, including the "no type script" result.
    for (i, cell) in rtx.resolved_inputs.iter().enumerate() {
        let output = &cell.cell_output;
        assert_eq!(
            cache.inputs[i].cached_lock_hash(),
            Some(&output.calc_lock_hash())
        );
        assert_eq!(
            cache.inputs[i].cached_type_hash(),
            Some(&output.type_().to_opt().map(|t| t.calc_script_hash()))
        );
    }
    for (i, output) in rtx.transaction.outputs().into_iter().enumerate() {
        assert_eq!(cache.outputs[i].cached_lock_hash(), None);
        assert_eq!(
            cache.outputs[i].cached_type_hash(),
            Some(&output.type_().to_opt().map(|t| t.calc_script_hash()))
        );
    }
    for (i, cell) in rtx.resolved_cell_deps.iter().enumerate() {
        let output = &cell.cell_output;
        assert_eq!(cache.cell_deps[i].cached_lock_hash(), None);
        assert_eq!(
            cache.cell_deps[i].cached_type_hash(),
            Some(&output.type_().to_opt().map(|t| t.calc_script_hash()))
        );
    }
}

#[test]
fn test_load_cell_script_hash_populates_cache_lazily() {
    let rtx = build_rtx();
    let sg_data = build_cached_sg_data(Arc::clone(&rtx));
    let cache = &sg_data.tx_info.script_hash_cache;

    let cell_dep_lock_hash = rtx.resolved_cell_deps[0].cell_output.calc_lock_hash();
    let result = load_cell_by_field(
        &sg_data,
        Source::Transaction(SourceEntry::CellDep),
        0,
        CellField::LockHash,
        32,
    );
    assert_eq!(result.data, cell_dep_lock_hash.as_slice());
    assert_eq!(
        cache.cell_deps[0].cached_lock_hash(),
        Some(&cell_dep_lock_hash)
    );
    assert_eq!(cache.cell_deps[1].cached_lock_hash(), None);

    // Group sources resolve to the cache entry of the actual output index.
    let sg_data = update_sg_info(sg_data, |sg_info| {
        sg_info.script_group.output_indices = vec![1];
    });
    let cache = &sg_data.tx_info.script_hash_cache;
    let output_lock_hash = rtx.transaction.outputs().get(1).unwrap().calc_lock_hash();
    let result = load_cell_by_field(
        &sg_data,
        Source::Group(SourceEntry::Output),
        0,
        CellField::LockHash,
        32,
    );
    assert_eq!(result.data, output_lock_hash.as_slice());
    assert_eq!(cache.outputs[0].cached_lock_hash(), None);
    assert_eq!(cache.outputs[1].cached_lock_hash(), Some(&output_lock_hash));
}

/// Once a hash is cached, the syscall must serve it from the cache instead of
/// hashing the script again. A sentinel value proves no recomputation occurs.
#[test]
fn test_load_cell_script_hash_served_from_cache() {
    let rtx = build_rtx();
    let sentinel_lock = Byte32::new([0xaa; 32]);
    let sentinel_type = Byte32::new([0xbb; 32]);
    let sg_data = update_tx_info(build_cached_sg_data(rtx), |tx_info| {
        let cache = &mut tx_info.script_hash_cache;
        for entries in [&mut cache.inputs, &mut cache.outputs, &mut cache.cell_deps] {
            entries[0] = CellScriptHashes::default()
                .with_lock_hash(sentinel_lock.clone())
                .with_type_hash(Some(sentinel_type.clone()));
        }
    });

    for (source, index) in ALL_SOURCES.into_iter().filter(|(_, index)| *index == 0) {
        let lock = load_cell_by_field(&sg_data, source, index, CellField::LockHash, 32);
        assert_eq!(lock.data, sentinel_lock.as_slice(), "{source:?}");
        let type_ = load_cell_by_field(&sg_data, source, index, CellField::TypeHash, 32);
        assert_eq!(type_.data, sentinel_type.as_slice(), "{source:?}");
    }
}
