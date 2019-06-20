use crate::{
    cost_model::instruction_cycles,
    syscalls::{
        Debugger, LoadCell, LoadCode, LoadHeader, LoadInput, LoadScriptHash, LoadTxHash,
        LoadWitness,
    },
    DataLoader, ScriptConfig, ScriptError,
};
use ckb_core::cell::{CellMeta, ResolvedOutPoint, ResolvedTransaction};
use ckb_core::extras::BlockExt;
use ckb_core::script::{Script, DAO_CODE_HASH};
use ckb_core::transaction::{CellInput, CellOutPoint, Witness};
use ckb_core::{BlockNumber, Capacity};
use ckb_core::{Bytes, Cycle};
use ckb_logger::info;
use ckb_resource::Resource;
use ckb_vm::{
    DefaultCoreMachine, DefaultMachineBuilder, SparseMemory, SupportMachine, TraceMachine,
    WXorXMemory,
};
use dao::calculate_maximum_withdraw;
use fnv::FnvHashMap;
use numext_fixed_hash::H256;
use std::cmp::min;

#[cfg(all(unix, target_pointer_width = "64"))]
use crate::Runner;
#[cfg(all(unix, target_pointer_width = "64"))]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};

pub const SYSTEM_DAO_CYCLES: u64 = 5000;

// TODO: tweak those values later
pub const DAO_LOCK_PERIOD_BLOCKS: BlockNumber = 10;
pub const DAO_MATURITY_BLOCKS: BlockNumber = 5;

// A script group is defined as scripts that share the same hash.
// A script group will only be executed once per transaction, the
// script itself should check against all inputs/outputs in its group
// if needed.
struct ScriptGroup {
    script: Script,
    input_indices: Vec<usize>,
    output_indices: Vec<usize>,
}

impl ScriptGroup {
    pub fn new(script: &Script) -> Self {
        Self {
            script: script.to_owned(),
            input_indices: vec![],
            output_indices: vec![],
        }
    }
}

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionScriptsVerifier<'a, DL> {
    data_loader: &'a DL,

    outputs: Vec<CellMeta>,
    rtx: &'a ResolvedTransaction<'a>,

    binary_index: FnvHashMap<H256, usize>,
    block_data: FnvHashMap<&'a H256, (BlockNumber, BlockExt)>,
    lock_groups: FnvHashMap<H256, ScriptGroup>,
    type_groups: FnvHashMap<H256, ScriptGroup>,

    // On windows we won't need this config right now, but removing it
    // on windows alone is too much effort comparing to simply allowing
    // it here.
    #[allow(dead_code)]
    config: &'a ScriptConfig,
}

impl<'a, DL: DataLoader> TransactionScriptsVerifier<'a, DL> {
    pub fn new(
        rtx: &'a ResolvedTransaction,
        data_loader: &'a DL,
        config: &'a ScriptConfig,
    ) -> TransactionScriptsVerifier<'a, DL> {
        let tx_hash = rtx.transaction.hash();
        let resolved_deps = &rtx.resolved_deps;
        let resolved_inputs = &rtx.resolved_inputs;
        let outputs = rtx
            .transaction
            .outputs()
            .iter()
            .enumerate()
            .map(|(index, output)| CellMeta {
                cell_output: Some(output.clone()),
                out_point: CellOutPoint {
                    tx_hash: tx_hash.to_owned(),
                    index: index as u32,
                },
                block_info: None,
                cellbase: false,
                capacity: output.capacity,
                data_hash: None,
            })
            .collect();

        let binary_index: FnvHashMap<H256, usize> = resolved_deps
            .iter()
            .enumerate()
            .map(|(i, dep_cell)| {
                if let Some(cell_meta) = &dep_cell.cell.cell_meta() {
                    let hash = match cell_meta.data_hash() {
                        Some(hash) => hash.to_owned(),
                        None => {
                            let output = data_loader.lazy_load_cell_output(cell_meta);
                            output.data_hash()
                        }
                    };
                    Some((hash, i))
                } else {
                    None
                }
            })
            .filter_map(|x| x)
            .collect();

        let mut block_data = FnvHashMap::<&'a H256, (BlockNumber, BlockExt)>::default();
        let mut lock_groups = FnvHashMap::default();
        let mut type_groups = FnvHashMap::default();
        for (i, resolved_input) in resolved_inputs.iter().enumerate() {
            if let Some(header) = &resolved_input.header {
                if let Some(block_ext) = data_loader.get_block_ext(header.hash()) {
                    block_data.insert(header.hash(), (header.number(), block_ext));
                }
            }
            // here we are only pre-processing the data, verify method validates
            // each input has correct script setup.
            if let Some(cell_meta) = resolved_input.cell.cell_meta() {
                let output = data_loader.lazy_load_cell_output(cell_meta);
                let lock_group_entry = lock_groups
                    .entry(output.lock.hash())
                    .or_insert_with(|| ScriptGroup::new(&output.lock));
                lock_group_entry.input_indices.push(i);
                if let Some(t) = output.type_ {
                    let type_group_entry = type_groups
                        .entry(t.hash())
                        .or_insert_with(|| ScriptGroup::new(&t));
                    type_group_entry.input_indices.push(i);
                }
            }
        }
        for (i, output) in rtx.transaction.outputs().iter().enumerate() {
            if let Some(t) = &output.type_ {
                let type_group_entry = type_groups
                    .entry(t.hash())
                    .or_insert_with(|| ScriptGroup::new(&t));
                type_group_entry.output_indices.push(i);
            }
        }
        for dep in resolved_deps {
            if let Some(header) = &dep.header {
                if let Some(block_ext) = data_loader.get_block_ext(header.hash()) {
                    block_data.insert(header.hash(), (header.number(), block_ext));
                }
            }
        }

        TransactionScriptsVerifier {
            data_loader,
            binary_index,
            block_data,
            outputs,
            rtx,
            config,
            lock_groups,
            type_groups,
        }
    }

    #[inline]
    fn inputs(&self) -> &[CellInput] {
        self.rtx.transaction.inputs()
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<ResolvedOutPoint> {
        &self.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_deps(&self) -> &Vec<ResolvedOutPoint> {
        &self.rtx.resolved_deps
    }

    #[inline]
    fn witnesses(&self) -> &[Witness] {
        self.rtx.transaction.witnesses()
    }

    #[inline]
    fn hash(&self) -> &H256 {
        self.rtx.transaction.hash()
    }

    fn build_load_tx_hash(&self) -> LoadTxHash {
        LoadTxHash::new(self.hash().as_bytes())
    }

    fn build_load_cell(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadCell<'a, DL> {
        LoadCell::new(
            &self.data_loader,
            &self.outputs,
            self.resolved_inputs(),
            self.resolved_deps(),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_code(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadCode<'a, DL> {
        LoadCode::new(
            &self.data_loader,
            &self.outputs,
            self.resolved_inputs(),
            self.resolved_deps(),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_input(&self, group_inputs: &'a [usize]) -> LoadInput {
        LoadInput::new(self.inputs(), group_inputs)
    }

    fn build_load_script_hash(&'a self, hash: &'a [u8]) -> LoadScriptHash<'a> {
        LoadScriptHash::new(hash)
    }

    fn build_load_header(&'a self, group_inputs: &'a [usize]) -> LoadHeader<'a> {
        LoadHeader::new(self.resolved_inputs(), self.resolved_deps(), group_inputs)
    }

    fn build_load_witness(&'a self, group_inputs: &'a [usize]) -> LoadWitness<'a> {
        LoadWitness::new(&self.witnesses(), group_inputs)
    }

    // Extracts actual script binary either in dep cells.
    fn extract_script(&self, script: &'a Script) -> Result<Bytes, ScriptError> {
        match self.binary_index.get(&script.code_hash).and_then(|index| {
            self.resolved_deps()[*index]
                .cell
                .cell_meta()
                .map(|cell_meta| self.data_loader.lazy_load_cell_output(&cell_meta))
        }) {
            Some(cell_output) => Ok(cell_output.data),
            None => Err(ScriptError::InvalidReferenceIndex),
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let mut cycles: Cycle = 0;
        // First, check if all inputs are resolved correctly
        for resolved_input in self.resolved_inputs() {
            if resolved_input.cell.is_issuing_dao_input() {
                if !self.valid_dao_withdraw_transaction() {
                    return Err(ScriptError::InvalidIssuingDaoInput);
                } else {
                    continue;
                }
            }
            if resolved_input.cell.cell_meta().is_none() {
                return Err(ScriptError::NoScript);
            }
        }

        // Now run each script group
        for group in self.lock_groups.values().chain(self.type_groups.values()) {
            let verify_result = if group.script.code_hash == DAO_CODE_HASH {
                self.verify_dao(&group, max_cycles)
            } else {
                let program = self.extract_script(&group.script)?;
                self.run(&program, &group, max_cycles)
            };
            let cycle = verify_result.map_err(|e| {
                info!(
                    "Error validating script group {:x} of transaction {:x}: {:?}",
                    group.script.hash(),
                    self.hash(),
                    e
                );
                e
            })?;
            let current_cycles = cycles
                .checked_add(cycle)
                .ok_or(ScriptError::ExceededMaximumCycles)?;
            if current_cycles > max_cycles {
                return Err(ScriptError::ExceededMaximumCycles);
            }
            cycles = current_cycles;
        }
        Ok(cycles)
    }

    fn verify_dao(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        // args should only contain pubkey hash
        let args = &script_group.script.args;
        if args.len() != 1 {
            return Err(ScriptError::ArgumentNumber);
        }

        let cycles = self
            .verify_default_lock(&script_group, max_cycles)?
            .checked_add(SYSTEM_DAO_CYCLES)
            .ok_or(ScriptError::ExceededMaximumCycles)?;;
        if cycles > max_cycles {
            return Err(ScriptError::ExceededMaximumCycles);
        }

        let mut maximum_output_capacities = Capacity::zero();
        for input_index in &script_group.input_indices {
            // Each DAO witness should contain 3 arguments in the following order:
            // 0. signature from witness
            // 1. withdraw block hash(32 bytes) from input args
            let witness = self
                .witnesses()
                .get(*input_index)
                .ok_or(ScriptError::NoWitness)?;
            if witness.len() != 2 {
                return Err(ScriptError::ArgumentNumber);
            }
            let withdraw_header_hash =
                H256::from_slice(&witness[1]).map_err(|_| ScriptError::IOError)?;
            let (withdraw_block_number, withdraw_block_ext) = self
                .block_data
                .get(&withdraw_header_hash)
                .ok_or(ScriptError::InvalidDaoWithdrawHeader)?;

            let input = self
                .inputs()
                .get(*input_index)
                .ok_or(ScriptError::ArgumentError)?;
            let resolved_input = self
                .resolved_inputs()
                .get(*input_index)
                .ok_or(ScriptError::ArgumentError)?;
            if resolved_input.cell().is_none() {
                return Err(ScriptError::ArgumentError);
            }
            let cell_meta = resolved_input.cell().unwrap();
            let output = self.data_loader.lazy_load_cell_output(&cell_meta);
            if output.lock.code_hash != DAO_CODE_HASH {
                return Err(ScriptError::ArgumentError);
            }

            if resolved_input.header().is_none() {
                return Err(ScriptError::InvalidDaoDepositHeader);
            }
            let (deposit_block_number, deposit_block_ext) = self
                .block_data
                .get(resolved_input.header().unwrap().hash())
                .ok_or(ScriptError::InvalidDaoDepositHeader)?;

            if withdraw_block_number <= deposit_block_number {
                return Err(ScriptError::InvalidDaoWithdrawHeader);
            }

            let windowleft = DAO_LOCK_PERIOD_BLOCKS
                - (withdraw_block_number - deposit_block_number) % DAO_LOCK_PERIOD_BLOCKS;
            let minimal_since = withdraw_block_number + min(DAO_MATURITY_BLOCKS, windowleft) + 1;

            if input.since < minimal_since {
                return Err(ScriptError::InvalidSince);
            }

            let maximum_withdraw = calculate_maximum_withdraw(
                &output,
                &deposit_block_ext.dao_stats,
                &withdraw_block_ext.dao_stats,
            )
            .map_err(|_| ScriptError::InterestCalculation)?;

            maximum_output_capacities = maximum_output_capacities
                .safe_add(maximum_withdraw)
                .map_err(|_| ScriptError::CapacityOverflow)?;
        }

        let output_capacities = self
            .outputs
            .iter()
            .map(|cell_meta| cell_meta.capacity)
            .try_fold(Capacity::zero(), Capacity::safe_add)
            .map_err(|_| ScriptError::CapacityOverflow)?;

        if output_capacities.as_u64() > maximum_output_capacities.as_u64() {
            return Err(ScriptError::InvalidInterest);
        }
        Ok(cycles)
    }

    // Default lock uses secp256k1_blake160_sighash_all now.
    fn verify_default_lock(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        // TODO: this is a temporary solution for now, later NervosDAO will be
        // implemented as a type script, while the default lock remains a lock
        // script.
        let program = Resource::bundled("specs/cells/secp256k1_blake160_sighash_all".to_string())
            .get()
            .map_err(|_| ScriptError::IOError)?;

        self.run(&program.into_owned().into(), &script_group, max_cycles)
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    fn run(
        &self,
        program: &Bytes,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        let current_script_hash = script_group.script.hash();
        let prefix = format!("script group: {:x}", current_script_hash);
        let current_script_hash_bytes = current_script_hash.as_bytes();
        let mut args = vec!["verify".into()];
        args.extend_from_slice(&script_group.script.args);
        let (code, cycles) = match self.config.runner {
            Runner::Assembly => {
                let core_machine = AsmCoreMachine::new_with_max_cycles(max_cycles);
                let machine = DefaultMachineBuilder::<Box<AsmCoreMachine>>::new(core_machine)
                    .instruction_cycle_func(Box::new(instruction_cycles))
                    .syscall(Box::new(
                        self.build_load_script_hash(current_script_hash_bytes),
                    ))
                    .syscall(Box::new(self.build_load_tx_hash()))
                    .syscall(Box::new(self.build_load_cell(
                        &script_group.input_indices,
                        &script_group.output_indices,
                    )))
                    .syscall(Box::new(self.build_load_input(&script_group.input_indices)))
                    .syscall(Box::new(
                        self.build_load_header(&script_group.input_indices),
                    ))
                    .syscall(Box::new(
                        self.build_load_witness(&script_group.input_indices),
                    ))
                    .syscall(Box::new(self.build_load_code(
                        &script_group.input_indices,
                        &script_group.output_indices,
                    )))
                    .syscall(Box::new(Debugger::new(&prefix)))
                    .build();
                let mut machine = AsmMachine::new(machine);
                machine
                    .load_program(&program, &args)
                    .map_err(ScriptError::VMError)?;
                let code = machine.run().map_err(ScriptError::VMError)?;
                (code, machine.machine.cycles())
            }
            Runner::Rust => {
                let core_machine =
                    DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::new_with_max_cycles(max_cycles);
                let machine = DefaultMachineBuilder::<
                    DefaultCoreMachine<u64, WXorXMemory<u64, SparseMemory<u64>>>,
                >::new(core_machine)
                .instruction_cycle_func(Box::new(instruction_cycles))
                .syscall(Box::new(
                    self.build_load_script_hash(current_script_hash_bytes),
                ))
                .syscall(Box::new(self.build_load_tx_hash()))
                .syscall(Box::new(self.build_load_cell(
                    &script_group.input_indices,
                    &script_group.output_indices,
                )))
                .syscall(Box::new(self.build_load_input(&script_group.input_indices)))
                .syscall(Box::new(
                    self.build_load_header(&script_group.input_indices),
                ))
                .syscall(Box::new(
                    self.build_load_witness(&script_group.input_indices),
                ))
                .syscall(Box::new(self.build_load_code(
                    &script_group.input_indices,
                    &script_group.output_indices,
                )))
                .syscall(Box::new(Debugger::new(&prefix)))
                .build();
                let mut machine = TraceMachine::new(machine);
                machine
                    .load_program(&program, &args)
                    .map_err(ScriptError::VMError)?;
                let code = machine.run().map_err(ScriptError::VMError)?;
                (code, machine.machine.cycles())
            }
        };
        if code == 0 {
            Ok(cycles)
        } else {
            Err(ScriptError::ValidationFailure(code))
        }
    }

    #[cfg(not(all(unix, target_pointer_width = "64")))]
    fn run(
        &self,
        program: &Bytes,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        let current_script_hash = script_group.script.hash();
        let prefix = format!("script group: {:x}", current_script_hash);
        let current_script_hash_bytes = current_script_hash.as_bytes();
        let mut args = vec!["verify".into()];
        args.extend_from_slice(&script_group.script.args);
        let core_machine =
            DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::new_with_max_cycles(
                max_cycles,
            );
        let machine = DefaultMachineBuilder::<
            DefaultCoreMachine<u64, WXorXMemory<u64, SparseMemory<u64>>>,
        >::new(core_machine)
        .instruction_cycle_func(Box::new(instruction_cycles))
        .syscall(Box::new(
            self.build_load_script_hash(current_script_hash_bytes),
        ))
        .syscall(Box::new(self.build_load_tx_hash()))
        .syscall(Box::new(self.build_load_cell(
            &script_group.input_indices,
            &script_group.output_indices,
        )))
        .syscall(Box::new(self.build_load_input(&script_group.input_indices)))
        .syscall(Box::new(
            self.build_load_header(&script_group.input_indices),
        ))
        .syscall(Box::new(
            self.build_load_witness(&script_group.input_indices),
        ))
        .syscall(Box::new(self.build_load_code(
            &script_group.input_indices,
            &script_group.output_indices,
        )))
        .syscall(Box::new(Debugger::new(&prefix)))
        .build();
        let mut machine = TraceMachine::new(machine);
        machine
            .load_program(&program, &args)
            .map_err(ScriptError::VMError)?;
        let code = machine.run().map_err(ScriptError::VMError)?;
        if code == 0 {
            Ok(machine.machine.cycles())
        } else {
            Err(ScriptError::ValidationFailure(code))
        }
    }

    fn valid_dao_withdraw_transaction(&self) -> bool {
        self.resolved_inputs().iter().any(|input| {
            input
                .cell
                .cell_meta()
                .map(|cell| {
                    let output = self.data_loader.lazy_load_cell_output(&cell);
                    output.lock.code_hash == DAO_CODE_HASH
                })
                .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(all(unix, target_pointer_width = "64")))]
    use crate::Runner;
    use ckb_core::cell::{BlockInfo, CellMetaBuilder};
    use ckb_core::extras::DaoStats;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::{capacity_bytes, Capacity};
    use ckb_db::MemoryKeyValueDB;
    use ckb_store::{
        data_loader_wrapper::DataLoaderWrapper, ChainKVStore, ChainStore, StoreBatch, COLUMNS,
    };
    use crypto::secp::{Generator, Privkey};
    use faster_hex::hex_encode;
    use hash::blake2b_256;

    use numext_fixed_hash::{h256, H256};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::sync::Arc;
    use test_chain_utils::create_always_success_cell;

    fn sha3_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
        tiny_keccak::sha3_256(s.as_ref())
    }

    fn open_cell_verify() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap()
    }

    fn new_memory_store() -> ChainKVStore<MemoryKeyValueDB> {
        ChainKVStore::new(MemoryKeyValueDB::open(COLUMNS as usize))
    }

    #[test]
    fn check_always_success_hash() {
        let (always_success_cell, always_success_script) = create_always_success_cell();
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            always_success_script.clone(),
            None,
        );
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(always_success_cell.clone())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![always_success_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100).is_ok());
    }

    #[test]
    fn check_signature() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_rust() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.clone().unwrap())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            &data_loader,
            &ScriptConfig {
                runner: Runner::Rust,
            },
        );

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    #[test]
    fn check_signature_assembly() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.clone().unwrap())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            &data_loader,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_with_not_enough_cycles() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100).is_err());
    }

    #[test]
    fn check_invalid_signature() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .data_hash(code_hash.to_owned())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);
        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);
        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_output_contract() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        args.push(Bytes::from(hex_signature));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_script) = create_always_success_cell();
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            always_success_script.clone(),
            None,
        );
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(always_success_cell.clone())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let script = Script::new(args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(
            Capacity::zero(),
            Bytes::default(),
            Script::new(vec![], H256::zero()),
            Some(script),
        );

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let dep_cell = {
            let output = CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                Bytes::from(buffer),
                Script::default(),
                None,
            );
            ResolvedOutPoint::cell_only(
                CellMetaBuilder::from_cell_output(output.to_owned())
                    .block_info(BlockInfo::new(1, 0))
                    .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                    .build(),
            )
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);
        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_invalid_output_contract() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_script) = create_always_success_cell();
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            always_success_script.clone(),
            None,
        );
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(always_success_cell.to_owned())
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let script = Script::new(args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(
            Capacity::zero(),
            Bytes::default(),
            Script::default(),
            Some(script),
        );

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let dep_cell = {
            let output = CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                Bytes::from(buffer),
                Script::default(),
                None,
            );
            ResolvedOutPoint::cell_only(
                CellMetaBuilder::from_cell_output(output)
                    .block_info(BlockInfo::new(1, 0))
                    .build(),
            )
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_tx_with_only_dao_issuing_input_but_no_dao_input() {
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            Script::default(),
            None,
        );
        let transaction = TransactionBuilder::default()
            .input(input)
            .output(output)
            .build();
        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![],
            resolved_inputs: vec![ResolvedOutPoint::issuing_dao()],
        };
        let store = Arc::new(new_memory_store());
        let data_loader = DataLoaderWrapper::new(store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_valid_dao_validation() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let pubkey = privkey.pubkey().unwrap().serialize();
        let pubkey_blake2b = blake2b_256(&pubkey);
        let pubkey_blake160 = &pubkey_blake2b[0..20];

        let deposit_header = HeaderBuilder::default()
            .number(1000)
            .transactions_root(h256!("0x1"))
            .build();
        let withdraw_header = HeaderBuilder::default()
            .number(1055)
            .transactions_root(h256!("0x2"))
            .build();
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
        );
        let deposit_output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::default(),
            Script::new(vec![pubkey_blake160.into()], DAO_CODE_HASH),
            None,
        );
        let withdraw_output = CellOutput::new(
            Capacity::shannons(100_000_000_009_999),
            Bytes::default(),
            Script::default(),
            None,
        );
        let transaction_hash = TransactionBuilder::default()
            .input(input.to_owned())
            .input(input2.to_owned())
            .output(withdraw_output.to_owned())
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .build()
            .hash()
            .to_owned();
        let mut message = vec![];
        message.write_all(&transaction_hash.as_bytes()).unwrap();
        message
            .write_all(&withdraw_header.hash().as_bytes())
            .unwrap();
        let signature = privkey
            .sign_recoverable(&blake2b_256(&message).into())
            .unwrap();
        let signature_ser = signature.serialize();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(signature_ser),
                Bytes::from(withdraw_header.hash().as_bytes()),
            ])
            .build();

        let store = Arc::new(new_memory_store());

        let deposit_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_000_123_456,
                ..Default::default()
            },
            ..Default::default()
        };
        let withdraw_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_001_123_456,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut batch = store.new_batch().unwrap();
        batch
            .insert_block_ext(deposit_header.hash(), &deposit_ext)
            .unwrap();
        batch
            .insert_block_ext(withdraw_header.hash(), &withdraw_ext)
            .unwrap();
        batch.commit().unwrap();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![ResolvedOutPoint::header_only(withdraw_header)],
            resolved_inputs: vec![
                ResolvedOutPoint::issuing_dao(),
                ResolvedOutPoint::cell_and_header((&deposit_output).into(), deposit_header),
            ],
        };

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_valid_dao_validation_with_fees() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let pubkey = privkey.pubkey().unwrap().serialize();
        let pubkey_blake2b = blake2b_256(&pubkey);
        let pubkey_blake160 = &pubkey_blake2b[0..20];

        let deposit_header = HeaderBuilder::default()
            .number(1000)
            .transactions_root(h256!("0x1"))
            .build();
        let withdraw_header = HeaderBuilder::default()
            .number(1055)
            .transactions_root(h256!("0x2"))
            .build();
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
        );
        let deposit_output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::default(),
            Script::new(vec![pubkey_blake160.into()], DAO_CODE_HASH),
            None,
        );
        let withdraw_output = CellOutput::new(
            Capacity::shannons(100_000_000_009_000),
            Bytes::default(),
            Script::default(),
            None,
        );
        let transaction_hash = TransactionBuilder::default()
            .input(input.to_owned())
            .input(input2.to_owned())
            .output(withdraw_output.to_owned())
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .build()
            .hash()
            .to_owned();
        let mut message = vec![];
        message.write_all(&transaction_hash.as_bytes()).unwrap();
        message
            .write_all(&withdraw_header.hash().as_bytes())
            .unwrap();
        let signature = privkey
            .sign_recoverable(&blake2b_256(&message).into())
            .unwrap();
        let signature_ser = signature.serialize();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(signature_ser),
                Bytes::from(withdraw_header.hash().as_bytes()),
            ])
            .build();

        let store = Arc::new(new_memory_store());

        let deposit_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_000_123_456,
                ..Default::default()
            },
            ..Default::default()
        };
        let withdraw_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_001_123_456,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut batch = store.new_batch().unwrap();
        batch
            .insert_block_ext(deposit_header.hash(), &deposit_ext)
            .unwrap();
        batch
            .insert_block_ext(withdraw_header.hash(), &withdraw_ext)
            .unwrap();
        batch.commit().unwrap();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![ResolvedOutPoint::header_only(withdraw_header)],
            resolved_inputs: vec![
                ResolvedOutPoint::issuing_dao(),
                ResolvedOutPoint::cell_and_header((&deposit_output).into(), deposit_header),
            ],
        };

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_valid_dao_validation_with_incorrect_secp() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let pubkey = privkey.pubkey().unwrap().serialize();
        let pubkey_blake2b = blake2b_256(&pubkey);
        let pubkey_blake160 = &pubkey_blake2b[0..20];

        let deposit_header = HeaderBuilder::default()
            .number(1000)
            .transactions_root(h256!("0x1"))
            .build();
        let withdraw_header = HeaderBuilder::default()
            .number(1055)
            .transactions_root(h256!("0x2"))
            .build();
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
        );
        let deposit_output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::default(),
            Script::new(vec![pubkey_blake160.into()], DAO_CODE_HASH),
            None,
        );
        let withdraw_output = CellOutput::new(
            Capacity::shannons(100_000_000_009_999),
            Bytes::default(),
            Script::default(),
            None,
        );
        let signature = privkey
            .sign_recoverable(&blake2b_256(&vec![1, 2, 3]).into())
            .unwrap();
        let signature_der = signature.serialize_der();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(withdraw_header.hash().as_bytes()),
            ])
            .build();

        let store = Arc::new(new_memory_store());

        let deposit_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_000_123_456,
                ..Default::default()
            },
            ..Default::default()
        };
        let withdraw_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_001_123_456,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut batch = store.new_batch().unwrap();
        batch
            .insert_block_ext(deposit_header.hash(), &deposit_ext)
            .unwrap();
        batch
            .insert_block_ext(withdraw_header.hash(), &withdraw_ext)
            .unwrap();
        batch.commit().unwrap();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![ResolvedOutPoint::header_only(withdraw_header)],
            resolved_inputs: vec![
                ResolvedOutPoint::issuing_dao(),
                ResolvedOutPoint::cell_and_header((&deposit_output).into(), deposit_header),
            ],
        };

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_valid_dao_validation_with_insufficient_cycles() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let pubkey = privkey.pubkey().unwrap().serialize();
        let pubkey_blake2b = blake2b_256(&pubkey);
        let pubkey_blake160 = &pubkey_blake2b[0..20];

        let deposit_header = HeaderBuilder::default()
            .number(1000)
            .transactions_root(h256!("0x1"))
            .build();
        let withdraw_header = HeaderBuilder::default()
            .number(1055)
            .transactions_root(h256!("0x2"))
            .build();
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
        );
        let deposit_output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::default(),
            Script::new(vec![pubkey_blake160.into()], DAO_CODE_HASH),
            None,
        );
        let withdraw_output = CellOutput::new(
            Capacity::shannons(100_000_000_009_999),
            Bytes::default(),
            Script::default(),
            None,
        );
        let transaction_hash = TransactionBuilder::default()
            .input(input.to_owned())
            .input(input2.to_owned())
            .output(withdraw_output.to_owned())
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .build()
            .hash()
            .to_owned();
        let mut message = vec![];
        message.write_all(&transaction_hash.as_bytes()).unwrap();
        message
            .write_all(&withdraw_header.hash().as_bytes())
            .unwrap();
        let signature = privkey
            .sign_recoverable(&blake2b_256(&message).into())
            .unwrap();
        let signature_der = signature.serialize_der();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(withdraw_header.hash().as_bytes()),
            ])
            .build();

        let store = Arc::new(new_memory_store());

        let deposit_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_000_123_456,
                ..Default::default()
            },
            ..Default::default()
        };
        let withdraw_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_001_123_456,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut batch = store.new_batch().unwrap();
        batch
            .insert_block_ext(deposit_header.hash(), &deposit_ext)
            .unwrap();
        batch
            .insert_block_ext(withdraw_header.hash(), &withdraw_ext)
            .unwrap();
        batch.commit().unwrap();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![ResolvedOutPoint::header_only(withdraw_header)],
            resolved_inputs: vec![
                ResolvedOutPoint::issuing_dao(),
                ResolvedOutPoint::cell_and_header((&deposit_output).into(), deposit_header),
            ],
        };

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100).is_err());
    }

    #[test]
    fn check_invalid_dao_withdraw_header() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let pubkey = privkey.pubkey().unwrap().serialize();
        let pubkey_blake2b = blake2b_256(&pubkey);
        let pubkey_blake160 = &pubkey_blake2b[0..20];

        let deposit_header = HeaderBuilder::default()
            .number(1000)
            .transactions_root(h256!("0x1"))
            .build();
        let withdraw_header = HeaderBuilder::default()
            .number(1055)
            .transactions_root(h256!("0x2"))
            .build();
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
        );
        let deposit_output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::default(),
            Script::new(vec![pubkey_blake160.into()], DAO_CODE_HASH),
            None,
        );
        let withdraw_output = CellOutput::new(
            Capacity::shannons(100_000_000_009_999),
            Bytes::default(),
            Script::default(),
            None,
        );
        let transaction_hash = TransactionBuilder::default()
            .input(input.to_owned())
            .input(input2.to_owned())
            .output(withdraw_output.to_owned())
            .build()
            .hash()
            .to_owned();
        let mut message = vec![];
        message.write_all(&transaction_hash.as_bytes()).unwrap();
        message
            .write_all(&withdraw_header.hash().as_bytes())
            .unwrap();
        let signature = privkey
            .sign_recoverable(&blake2b_256(&message).into())
            .unwrap();
        let signature_der = signature.serialize_der();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(withdraw_header.hash().as_bytes()),
            ])
            .build();

        let store = Arc::new(new_memory_store());

        let deposit_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_000_123_456,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut batch = store.new_batch().unwrap();
        batch
            .insert_block_ext(deposit_header.hash(), &deposit_ext)
            .unwrap();
        batch.commit().unwrap();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![ResolvedOutPoint::header_only(withdraw_header)],
            resolved_inputs: vec![
                ResolvedOutPoint::issuing_dao(),
                ResolvedOutPoint::cell_and_header((&deposit_output).into(), deposit_header),
            ],
        };

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_dao_maximum_withdraw_value() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let pubkey = privkey.pubkey().unwrap().serialize();
        let pubkey_blake2b = blake2b_256(&pubkey);
        let pubkey_blake160 = &pubkey_blake2b[0..20];

        let deposit_header = HeaderBuilder::default()
            .number(1000)
            .transactions_root(h256!("0x1"))
            .build();
        let withdraw_header = HeaderBuilder::default()
            .number(1055)
            .transactions_root(h256!("0x2"))
            .build();
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
        );
        let deposit_output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::default(),
            Script::new(vec![pubkey_blake160.into()], DAO_CODE_HASH),
            None,
        );
        let withdraw_output = CellOutput::new(
            Capacity::shannons(100_000_000_010_000),
            Bytes::default(),
            Script::default(),
            None,
        );
        let transaction_hash = TransactionBuilder::default()
            .input(input.to_owned())
            .input(input2.to_owned())
            .output(withdraw_output.to_owned())
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .build()
            .hash()
            .to_owned();
        let mut message = vec![];
        message.write_all(&transaction_hash.as_bytes()).unwrap();
        message
            .write_all(&withdraw_header.hash().as_bytes())
            .unwrap();
        let signature = privkey
            .sign_recoverable(&blake2b_256(&message).into())
            .unwrap();
        let signature_der = signature.serialize_der();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(withdraw_header.hash().as_bytes()),
            ])
            .build();

        let store = Arc::new(new_memory_store());

        let deposit_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_000_123_456,
                ..Default::default()
            },
            ..Default::default()
        };
        let withdraw_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_001_123_456,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut batch = store.new_batch().unwrap();
        batch
            .insert_block_ext(deposit_header.hash(), &deposit_ext)
            .unwrap();
        batch
            .insert_block_ext(withdraw_header.hash(), &withdraw_ext)
            .unwrap();
        batch.commit().unwrap();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![ResolvedOutPoint::header_only(withdraw_header)],
            resolved_inputs: vec![
                ResolvedOutPoint::issuing_dao(),
                ResolvedOutPoint::cell_and_header((&deposit_output).into(), deposit_header),
            ],
        };

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_dao_since() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let pubkey = privkey.pubkey().unwrap().serialize();
        let pubkey_blake2b = blake2b_256(&pubkey);
        let pubkey_blake160 = &pubkey_blake2b[0..20];

        let deposit_header = HeaderBuilder::default()
            .number(1000)
            .transactions_root(h256!("0x1"))
            .build();
        let withdraw_header = HeaderBuilder::default()
            .number(1055)
            .transactions_root(h256!("0x2"))
            .build();
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1060,
        );
        let deposit_output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::default(),
            Script::new(vec![pubkey_blake160.into()], DAO_CODE_HASH),
            None,
        );
        let withdraw_output = CellOutput::new(
            Capacity::shannons(100_000_000_009_999),
            Bytes::default(),
            Script::default(),
            None,
        );
        let transaction_hash = TransactionBuilder::default()
            .input(input.to_owned())
            .input(input2.to_owned())
            .output(withdraw_output.to_owned())
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .build()
            .hash()
            .to_owned();
        let mut message = vec![];
        message.write_all(&transaction_hash.as_bytes()).unwrap();
        message
            .write_all(&withdraw_header.hash().as_bytes())
            .unwrap();
        let signature = privkey
            .sign_recoverable(&blake2b_256(&message).into())
            .unwrap();
        let signature_der = signature.serialize_der();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(withdraw_header.hash().as_bytes()),
            ])
            .build();

        let store = Arc::new(new_memory_store());

        let deposit_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_000_123_456,
                ..Default::default()
            },
            ..Default::default()
        };
        let withdraw_ext = BlockExt {
            dao_stats: DaoStats {
                accumulated_rate: 10_000_000_001_123_456,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut batch = store.new_batch().unwrap();
        batch
            .insert_block_ext(deposit_header.hash(), &deposit_ext)
            .unwrap();
        batch
            .insert_block_ext(withdraw_header.hash(), &withdraw_ext)
            .unwrap();
        batch.commit().unwrap();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![ResolvedOutPoint::header_only(withdraw_header)],
            resolved_inputs: vec![
                ResolvedOutPoint::issuing_dao(),
                ResolvedOutPoint::cell_and_header((&deposit_output).into(), deposit_header),
            ],
        };

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_same_lock_and_type_script_are_executed_twice() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let privkey = Privkey::from_slice(&[1; 32][..]);
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.push(Bytes::from(hex_signature));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(args, code_hash.to_owned());

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let transaction = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), 0))
            .dep(dep_out_point)
            .build();

        // The lock and type scripts here are both executed.
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            script.clone(),
            Some(script.clone()),
        );
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_info(BlockInfo::new(1, 0))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = Arc::new(new_memory_store());

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let data_loader = DataLoaderWrapper::new(store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        // Cycles can tell that both lock and type scripts are executed
        assert_eq!(verifier.verify(100_000_000), Ok(2_818_104));
    }
}
