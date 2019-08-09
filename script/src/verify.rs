use crate::{
    cost_model::instruction_cycles,
    syscalls::{
        Debugger, LoadCell, LoadCellData, LoadHeader, LoadInput, LoadScriptHash, LoadTxHash,
        LoadWitness,
    },
    type_id::{TypeIdSystemScript, TYPE_ID_CODE_HASH},
    DataLoader, ScriptConfig, ScriptError,
};
use ckb_core::cell::{CellMeta, ResolvedOutPoint, ResolvedTransaction};
use ckb_core::script::{Script, ScriptHashType};
use ckb_core::transaction::{CellInput, CellOutPoint, Witness};
use ckb_core::{Bytes, Cycle};
use ckb_logger::{debug, info};
use ckb_vm::{
    DefaultCoreMachine, DefaultMachineBuilder, SparseMemory, SupportMachine, TraceMachine,
    WXorXMemory,
};
use fnv::FnvHashMap;
use numext_fixed_hash::H256;

#[cfg(all(unix, target_pointer_width = "64"))]
use crate::Runner;
#[cfg(all(unix, target_pointer_width = "64"))]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};

// A script group is defined as scripts that share the same hash.
// A script group will only be executed once per transaction, the
// script itself should check against all inputs/outputs in its group
// if needed.
pub struct ScriptGroup {
    pub script: Script,
    pub input_indices: Vec<usize>,
    pub output_indices: Vec<usize>,
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
    debug_printer: Option<Box<dyn Fn(&H256, &str)>>,

    outputs: Vec<CellMeta>,
    rtx: &'a ResolvedTransaction<'a>,

    binaries_by_data_hash: FnvHashMap<H256, Bytes>,
    binaries_by_type_hash: FnvHashMap<H256, (Bytes, bool)>,
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
            .outputs_with_data_iter()
            .enumerate()
            .map(|(index, (output, data))| CellMeta {
                cell_output: output.to_owned(),
                out_point: CellOutPoint {
                    tx_hash: tx_hash.to_owned(),
                    index: index as u32,
                },
                block_info: None,
                cellbase: false,
                data_bytes: data.len() as u64,
                mem_cell_data: Some(data.to_owned()),
            })
            .collect();

        let mut binaries_by_data_hash: FnvHashMap<H256, Bytes> = FnvHashMap::default();
        let mut binaries_by_type_hash: FnvHashMap<H256, (Bytes, bool)> = FnvHashMap::default();
        for resolved_dep in resolved_deps {
            if let Some(cell_meta) = &resolved_dep.cell() {
                let data = data_loader.load_cell_data(cell_meta).expect("cell data");
                binaries_by_data_hash.insert(cell_meta.data_hash().to_owned(), data.to_owned());
                if let Some(t) = &cell_meta.cell_output.type_ {
                    binaries_by_type_hash
                        .entry(t.hash())
                        .and_modify(|e| e.1 = true)
                        .or_insert((data.to_owned(), false));
                }
            }
        }

        let mut lock_groups = FnvHashMap::default();
        let mut type_groups = FnvHashMap::default();
        for (i, resolved_input) in resolved_inputs.iter().enumerate() {
            // here we are only pre-processing the data, verify method validates
            // each input has correct script setup.
            if let Some(cell_meta) = resolved_input.cell() {
                let output = &cell_meta.cell_output;
                let lock_group_entry = lock_groups
                    .entry(output.lock.hash())
                    .or_insert_with(|| ScriptGroup::new(&output.lock));
                lock_group_entry.input_indices.push(i);
                if let Some(t) = &output.type_ {
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

        TransactionScriptsVerifier {
            data_loader,
            binaries_by_data_hash,
            binaries_by_type_hash,
            outputs,
            rtx,
            config,
            lock_groups,
            type_groups,
            debug_printer: None,
        }
    }

    pub fn set_debug_printer<F: Fn(&H256, &str) + 'static>(&mut self, func: F) {
        self.debug_printer = Some(Box::new(func));
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
    ) -> LoadCell<'a> {
        LoadCell::new(
            &self.outputs,
            self.resolved_inputs(),
            self.resolved_deps(),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_cell_data(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadCellData<'a, DL> {
        LoadCellData::new(
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
        match script.hash_type {
            ScriptHashType::Data => {
                if let Some(data) = self.binaries_by_data_hash.get(&script.code_hash) {
                    Ok(data.to_owned())
                } else {
                    Err(ScriptError::InvalidCodeHash)
                }
            }
            ScriptHashType::Type => {
                if let Some((data, multiple)) = self.binaries_by_type_hash.get(&script.code_hash) {
                    if *multiple {
                        Err(ScriptError::MultipleMatches)
                    } else {
                        Ok(data.to_owned())
                    }
                } else {
                    Err(ScriptError::InvalidCodeHash)
                }
            }
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let mut cycles: Cycle = 0;
        // Check if all inputs are resolved correctly
        if self
            .resolved_inputs()
            .iter()
            .any(|input| input.cell.is_none())
        {
            return Err(ScriptError::NoScript);
        }

        // Now run each script group
        for group in self.lock_groups.values().chain(self.type_groups.values()) {
            let result = if group.script.code_hash == TYPE_ID_CODE_HASH
                && group.script.hash_type == ScriptHashType::Type
            {
                let verifier = TypeIdSystemScript {
                    rtx: self.rtx,
                    script_group: group,
                    max_cycles,
                };
                verifier.verify()
            } else {
                let program = self.extract_script(&group.script)?;
                self.run(&program, &group, max_cycles)
            };
            let cycle = result.map_err(|e| {
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

    #[cfg(all(unix, target_pointer_width = "64"))]
    fn run(
        &self,
        program: &Bytes,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        let current_script_hash = script_group.script.hash();
        let prefix = format!("script group: {:x}", current_script_hash);
        let debug_printer = |message: &str| {
            if let Some(ref printer) = self.debug_printer {
                printer(&current_script_hash, message);
            } else {
                debug!("{} DEBUG OUTPUT: {}", prefix, message);
            };
        };
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
                    .syscall(Box::new(self.build_load_cell_data(
                        &script_group.input_indices,
                        &script_group.output_indices,
                    )))
                    .syscall(Box::new(Debugger::new(&debug_printer)))
                    .build();
                let mut machine = AsmMachine::new(machine, None);
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
                .syscall(Box::new(self.build_load_cell_data(
                    &script_group.input_indices,
                    &script_group.output_indices,
                )))
                .syscall(Box::new(Debugger::new(&debug_printer)))
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
        let debug_printer = |message: &str| {
            if let Some(ref printer) = self.debug_printer {
                printer(&current_script_hash, message);
            } else {
                debug!("{} DEBUG OUTPUT: {}", prefix, message);
            };
        };
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
        .syscall(Box::new(self.build_load_cell_data(
            &script_group.input_indices,
            &script_group.output_indices,
        )))
        .syscall(Box::new(Debugger::new(&debug_printer)))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(all(unix, target_pointer_width = "64")))]
    use crate::Runner;
    use byteorder::{ByteOrder, LittleEndian};
    use ckb_core::cell::{BlockInfo, CellMetaBuilder};
    use ckb_core::script::{Script, ScriptHashType};
    use ckb_core::transaction::{
        CellInput, CellOutput, CellOutputBuilder, OutPoint, TransactionBuilder,
    };
    use ckb_core::{capacity_bytes, Capacity};
    use ckb_crypto::secp::{Generator, Privkey, Pubkey, Signature};
    use ckb_db::RocksDB;
    use ckb_hash::{blake2b_256, new_blake2b};
    use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainDB, COLUMNS};
    use faster_hex::hex_encode;

    use ckb_test_chain_utils::always_success_cell;
    use ckb_vm::Error as VMInternalError;
    use numext_fixed_hash::{h256, H256};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;

    fn sha3_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
        tiny_keccak::sha3_256(s.as_ref())
    }

    fn open_cell_verify() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap()
    }

    fn new_store() -> ChainDB {
        ChainDB::new(RocksDB::open_tmp(COLUMNS))
    }

    fn random_keypair() -> (Privkey, Pubkey) {
        let gen = Generator::new();
        gen.random_keypair()
    }

    fn to_hex_pubkey(pubkey: &Pubkey) -> Vec<u8> {
        let pubkey = pubkey.serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        hex_pubkey
    }

    fn to_hex_signature(signature: &Signature) -> Vec<u8> {
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        hex_signature
    }

    fn sign_args(args: &[Bytes], privkey: &Privkey) -> Signature {
        let mut bytes = vec![];
        for argument in args.iter() {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        privkey.sign_recoverable(&hash2.into()).unwrap()
    }

    #[test]
    fn check_always_success_hash() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(always_success_script.clone())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .block_info(BlockInfo::new(1, 0, H256::zero()))
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![always_success_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

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

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let signature = sign_args(&args, &privkey);
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::from_data(&data)
            .capacity(Capacity::bytes(data.len()).unwrap())
            .build();
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, data)
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, code_hash, ScriptHashType::Data);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(script)
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        // Default Runner
        assert!(verifier.verify(100_000_000).is_ok());

        // Not enought cycles
        assert_eq!(
            verifier.verify(100).err(),
            Some(ScriptError::VMError(VMInternalError::InvalidCycles))
        );

        // Rust Runner
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

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let signature = sign_args(&args, &privkey);
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::from_data(&data)
            .capacity(Capacity::bytes(data.len()).unwrap())
            .build();
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, data)
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .out_point(dep_out_point.cell.clone().unwrap())
                .build(),
        );

        let script = Script::new(args, code_hash, ScriptHashType::Data);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(script)
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

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
    fn check_signature_referenced_via_type_hash() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let signature = sign_args(&args, &privkey);
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::from_data(&data)
            .capacity(Capacity::bytes(data.len()).unwrap())
            .type_(Some(Script::new(
                vec![],
                h256!("0x123456abcd90"),
                ScriptHashType::Data,
            )))
            .build();
        let type_hash: H256 = output.type_.as_ref().unwrap().hash();
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, data)
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, type_hash, ScriptHashType::Type);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(script)
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_referenced_via_type_hash_failure_with_multiple_matches() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();
        let data = Bytes::from(buffer);

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let signature = sign_args(&args, &privkey);
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let output = CellOutputBuilder::from_data(&data.clone())
            .capacity(Capacity::bytes(data.len()).unwrap())
            .type_(Some(Script::new(
                vec![],
                h256!("0x123456abcd90"),
                ScriptHashType::Data,
            )))
            .build();
        let type_hash: H256 = output.type_.as_ref().unwrap().hash();
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, data.clone())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let dep_out_point2 = OutPoint::new_cell(h256!("0x1234"), 8);
        let output2 = CellOutputBuilder::from_data(&data.clone())
            .capacity(Capacity::bytes(data.len()).unwrap())
            .type_(Some(Script::new(
                vec![],
                h256!("0x123456abcd90"),
                ScriptHashType::Data,
            )))
            .build();
        let dep_cell2 = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output2, data)
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .out_point(dep_out_point2.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, type_hash, ScriptHashType::Type);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .dep(dep_out_point2)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(script)
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell, dep_cell2],
            resolved_inputs: vec![dummy_cell],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(100_000_000),
            Err(ScriptError::MultipleMatches)
        );
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
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::from_data(&data)
            .capacity(Capacity::bytes(data.len()).unwrap())
            .build();
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned(), data)
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, code_hash, ScriptHashType::Data);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(script)
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

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

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let signature = sign_args(&args, &privkey);

        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::from_data(&data)
            .capacity(Capacity::bytes(data.len()).unwrap())
            .build();
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned(), data)
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let script = Script::new(args, code_hash, ScriptHashType::Data);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(script)
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(100_000_000).err(),
            Some(ScriptError::ValidationFailure(2))
        );
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let signature = sign_args(&args, &privkey);
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(args, code_hash, ScriptHashType::Data);
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(script)
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![],
            resolved_inputs: vec![dummy_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(100_000_000).err(),
            Some(ScriptError::InvalidCodeHash)
        );
    }

    #[test]
    fn check_output_contract() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let signature = sign_args(&args, &privkey);
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .block_info(BlockInfo::new(1, 0, H256::zero()))
            .build(),
        );

        let script = Script::new(args, (&blake2b_256(&buffer)).into(), ScriptHashType::Data);
        let output_data = Bytes::default();
        let output = CellOutputBuilder::default()
            .lock(Script::new(vec![], H256::zero(), ScriptHashType::Data))
            .type_(Some(script))
            .build();

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let dep_cell = {
            let data = Bytes::from(buffer);
            let output = CellOutputBuilder::from_data(&data)
                .capacity(Capacity::bytes(data.len()).unwrap())
                .build();
            ResolvedOutPoint::cell_only(
                CellMetaBuilder::from_cell_output(output.to_owned(), data)
                    .block_info(BlockInfo::new(1, 0, H256::zero()))
                    .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                    .build(),
            )
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .output_data(output_data.clone())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
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

        let (privkey, pubkey) = random_keypair();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let signature = sign_args(&args, &privkey);
        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.to_owned(),
                always_success_cell_data.to_owned(),
            )
            .block_info(BlockInfo::new(1, 0, H256::zero()))
            .build(),
        );

        let script = Script::new(args, (&blake2b_256(&buffer)).into(), ScriptHashType::Data);
        let output = CellOutputBuilder::default().type_(Some(script)).build();

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let dep_cell = {
            let dep_cell_data = Bytes::from(buffer);
            let output = CellOutputBuilder::from_data(&dep_cell_data)
                .capacity(Capacity::bytes(dep_cell_data.len()).unwrap())
                .build();
            ResolvedOutPoint::cell_only(
                CellMetaBuilder::from_cell_output(output, dep_cell_data)
                    .block_info(BlockInfo::new(1, 0, H256::zero()))
                    .build(),
            )
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .output_data(Bytes::new())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(100_000_000).err(),
            Some(ScriptError::ValidationFailure(2))
        );
    }

    #[test]
    fn check_same_lock_and_type_script_are_executed_twice() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let privkey = Privkey::from_slice(&[1; 32][..]);
        let pubkey = privkey.pubkey().unwrap();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let signature = sign_args(&args, &privkey);
        args.push(Bytes::from(to_hex_pubkey(&pubkey)));
        args.push(Bytes::from(to_hex_signature(&signature)));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(args, code_hash.to_owned(), ScriptHashType::Data);

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::from_data(&data)
            .capacity(Capacity::bytes(data.len()).unwrap())
            .build();
        let dep_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, data)
                .block_info(BlockInfo::new(1, 0, H256::zero()))
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
            H256::zero(),
            script.clone(),
            Some(script.clone()),
        );
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output, Bytes::new())
                .block_info(BlockInfo::new(1, 0, H256::zero()))
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        // Cycles can tell that both lock and type scripts are executed
        assert_eq!(verifier.verify(100_000_000), Ok(2_818_104));
    }

    #[test]
    fn check_type_id_one_in_one_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new_cell(h256!("0x11"), 0);

        let type_id_script = Script::new(
            vec![Bytes::from(&h256!("0x1111")[..])],
            TYPE_ID_CODE_HASH,
            ScriptHashType::Type,
        );

        let input = CellInput::new(OutPoint::new_cell(h256!("0x1234"), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .dep(always_success_out_point.clone())
            .build();

        let resolved_input_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
                .out_point(input.previous_output.cell.clone().unwrap())
                .build(),
        );
        let resolved_always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .out_point(always_success_out_point.cell.clone().unwrap())
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_one_in_one_out_not_enough_cycles() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new_cell(h256!("0x11"), 0);

        let type_id_script = Script::new(
            vec![Bytes::from(&h256!("0x1111")[..])],
            TYPE_ID_CODE_HASH,
            ScriptHashType::Type,
        );

        let input = CellInput::new(OutPoint::new_cell(h256!("0x1234"), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .dep(always_success_out_point.clone())
            .build();

        let resolved_input_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
                .out_point(input.previous_output.cell.clone().unwrap())
                .build(),
        );
        let resolved_always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .out_point(always_success_out_point.cell.clone().unwrap())
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(500_000).err(),
            Some(ScriptError::ExceededMaximumCycles)
        );
    }

    #[test]
    fn check_type_id_creation() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new_cell(h256!("0x11"), 0);

        let input = CellInput::new(OutPoint::new_cell(h256!("0x1234"), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000))
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(
                input
                    .previous_output
                    .cell
                    .as_ref()
                    .unwrap()
                    .tx_hash
                    .as_bytes(),
            );
            let mut buf = [0; 4];
            LittleEndian::write_u32(&mut buf, input.previous_output.cell.as_ref().unwrap().index);
            blake2b.update(&buf[..]);
            let mut buf = [0; 8];
            LittleEndian::write_u64(&mut buf, 0);
            blake2b.update(&buf[..]);
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            Bytes::from(&ret[..])
        };

        let type_id_script = Script::new(vec![input_hash], TYPE_ID_CODE_HASH, ScriptHashType::Type);

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .dep(always_success_out_point.clone())
            .build();

        let resolved_input_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
                .out_point(input.previous_output.cell.clone().unwrap())
                .build(),
        );
        let resolved_always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .out_point(always_success_out_point.cell.clone().unwrap())
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_termination() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new_cell(h256!("0x11"), 0);

        let type_id_script = Script::new(
            vec![Bytes::from(&h256!("0x1111")[..])],
            TYPE_ID_CODE_HASH,
            ScriptHashType::Type,
        );

        let input = CellInput::new(OutPoint::new_cell(h256!("0x1234"), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .dep(always_success_out_point.clone())
            .build();

        let resolved_input_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
                .out_point(input.previous_output.cell.clone().unwrap())
                .build(),
        );
        let resolved_always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .out_point(always_success_out_point.cell.clone().unwrap())
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_invalid_creation() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new_cell(h256!("0x11"), 0);

        let input = CellInput::new(OutPoint::new_cell(h256!("0x1234"), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000))
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(
                input
                    .previous_output
                    .cell
                    .as_ref()
                    .unwrap()
                    .tx_hash
                    .as_bytes(),
            );
            let mut buf = [0; 4];
            LittleEndian::write_u32(&mut buf, input.previous_output.cell.as_ref().unwrap().index);
            blake2b.update(&buf[..]);
            let mut buf = [0; 8];
            LittleEndian::write_u64(&mut buf, 0);
            blake2b.update(&buf[..]);
            blake2b.update(b"unnecessary data");
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            Bytes::from(&ret[..])
        };

        let type_id_script = Script::new(vec![input_hash], TYPE_ID_CODE_HASH, ScriptHashType::Type);

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .dep(always_success_out_point.clone())
            .build();

        let resolved_input_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
                .out_point(input.previous_output.cell.clone().unwrap())
                .build(),
        );
        let resolved_always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .out_point(always_success_out_point.cell.clone().unwrap())
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(1_001_000).err(),
            Some(ScriptError::ValidationFailure(-3))
        );
    }

    #[test]
    fn check_type_id_invalid_creation_length() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new_cell(h256!("0x11"), 0);

        let input = CellInput::new(OutPoint::new_cell(h256!("0x1234"), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000))
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(
                input
                    .previous_output
                    .cell
                    .as_ref()
                    .unwrap()
                    .tx_hash
                    .as_bytes(),
            );
            let mut buf = [0; 4];
            LittleEndian::write_u32(&mut buf, input.previous_output.cell.as_ref().unwrap().index);
            blake2b.update(&buf[..]);
            let mut buf = [0; 8];
            LittleEndian::write_u64(&mut buf, 0);
            blake2b.update(&buf[..]);
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);

            let mut buf = vec![];
            buf.extend_from_slice(&ret[..]);
            buf.extend_from_slice(b"abc");
            Bytes::from(&buf[..])
        };

        let type_id_script = Script::new(vec![input_hash], TYPE_ID_CODE_HASH, ScriptHashType::Type);

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .dep(always_success_out_point.clone())
            .build();

        let resolved_input_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
                .out_point(input.previous_output.cell.clone().unwrap())
                .build(),
        );
        let resolved_always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .out_point(always_success_out_point.cell.clone().unwrap())
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(1_001_000).err(),
            Some(ScriptError::ValidationFailure(-1))
        );
    }

    #[test]
    fn check_type_id_one_in_two_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new_cell(h256!("0x11"), 0);

        let type_id_script = Script::new(
            vec![Bytes::from(&h256!("0x1111")[..])],
            TYPE_ID_CODE_HASH,
            ScriptHashType::Type,
        );

        let input = CellInput::new(OutPoint::new_cell(h256!("0x1234"), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(2000))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();
        let output_cell2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990))
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()))
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .output(output_cell2)
            .dep(always_success_out_point.clone())
            .build();

        let resolved_input_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
                .out_point(input.previous_output.cell.clone().unwrap())
                .build(),
        );
        let resolved_always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(
                always_success_cell.clone(),
                always_success_cell_data.to_owned(),
            )
            .out_point(always_success_out_point.cell.clone().unwrap())
            .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let config = ScriptConfig {
            runner: Runner::default(),
        };
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader, &config);

        assert_eq!(
            verifier.verify(1_001_000).err(),
            Some(ScriptError::ValidationFailure(-2))
        );
    }
}
