use crate::{
    cost_model::instruction_cycles,
    syscalls::{
        Debugger, LoadCell, LoadCellData, LoadHeader, LoadInput, LoadScriptHash, LoadTxHash,
        LoadWitness,
    },
    type_id::TypeIdSystemScript,
    DataLoader, ScriptError,
};
use ckb_error::{Error, InternalErrorKind};
#[cfg(feature = "logging")]
use ckb_logger::{debug, info};
use ckb_types::{
    bytes::Bytes,
    constants::TYPE_ID_CODE_HASH,
    core::{
        cell::{CellMeta, ResolvedTransaction},
        Cycle, ScriptHashType,
    },
    packed::{Byte32, Byte32Vec, CellInputVec, CellOutput, OutPoint, Script, WitnessVec},
    prelude::*,
};
#[cfg(has_asm)]
use ckb_vm::{
    machine::asm::{AsmCoreMachine, AsmMachine},
    DefaultMachineBuilder, SupportMachine,
};
#[cfg(not(has_asm))]
use ckb_vm::{
    DefaultCoreMachine, DefaultMachineBuilder, SparseMemory, SupportMachine, TraceMachine,
    WXorXMemory,
};
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGroupType {
    Lock,
    Type,
}

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionScriptsVerifier<'a, DL> {
    data_loader: &'a DL,
    debug_printer: Option<Box<dyn Fn(&Byte32, &str)>>,

    outputs: Vec<CellMeta>,
    rtx: &'a ResolvedTransaction,

    binaries_by_data_hash: HashMap<Byte32, Bytes>,
    binaries_by_type_hash: HashMap<Byte32, (Bytes, bool)>,
    lock_groups: HashMap<Byte32, ScriptGroup>,
    type_groups: HashMap<Byte32, ScriptGroup>,
}

impl<'a, DL: DataLoader> TransactionScriptsVerifier<'a, DL> {
    pub fn new(
        rtx: &'a ResolvedTransaction,
        data_loader: &'a DL,
    ) -> TransactionScriptsVerifier<'a, DL> {
        let tx_hash = rtx.transaction.hash();
        let resolved_cell_deps = &rtx.resolved_cell_deps;
        let resolved_inputs = &rtx.resolved_inputs;
        let outputs = rtx
            .transaction
            .outputs_with_data_iter()
            .enumerate()
            .map(|(index, (cell_output, data))| {
                let out_point = OutPoint::new_builder()
                    .tx_hash(tx_hash.clone())
                    .index(index.pack())
                    .build();
                let data_hash = CellOutput::calc_data_hash(&data);
                CellMeta {
                    cell_output,
                    out_point,
                    transaction_info: None,
                    data_bytes: data.len() as u64,
                    mem_cell_data: Some((data, data_hash)),
                }
            })
            .collect();

        let mut binaries_by_data_hash: HashMap<Byte32, Bytes> = HashMap::default();
        let mut binaries_by_type_hash: HashMap<Byte32, (Bytes, bool)> = HashMap::default();
        for cell_meta in resolved_cell_deps {
            let (data, data_hash) = data_loader.load_cell_data(cell_meta).expect("cell data");
            binaries_by_data_hash.insert(data_hash, data.to_owned());
            if let Some(t) = &cell_meta.cell_output.type_().to_opt() {
                binaries_by_type_hash
                    .entry(t.calc_script_hash())
                    .and_modify(|e| e.1 = true)
                    .or_insert((data.to_owned(), false));
            }
        }

        let mut lock_groups = HashMap::default();
        let mut type_groups = HashMap::default();
        for (i, cell_meta) in resolved_inputs.iter().enumerate() {
            // here we are only pre-processing the data, verify method validates
            // each input has correct script setup.
            let output = &cell_meta.cell_output;
            let lock_group_entry = lock_groups
                .entry(output.calc_lock_hash())
                .or_insert_with(|| ScriptGroup::new(&output.lock()));
            lock_group_entry.input_indices.push(i);
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::new(&t));
                type_group_entry.input_indices.push(i);
            }
        }
        for (i, output) in rtx.transaction.outputs().into_iter().enumerate() {
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
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
            lock_groups,
            type_groups,
            debug_printer: None,
        }
    }

    pub fn set_debug_printer<F: Fn(&Byte32, &str) + 'static>(&mut self, func: F) {
        self.debug_printer = Some(Box::new(func));
    }

    #[inline]
    fn inputs(&self) -> CellInputVec {
        self.rtx.transaction.inputs()
    }

    #[inline]
    fn header_deps(&self) -> Byte32Vec {
        self.rtx.transaction.header_deps()
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_cell_deps(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_cell_deps
    }

    #[inline]
    fn witnesses(&self) -> WitnessVec {
        self.rtx.transaction.witnesses()
    }

    #[inline]
    fn hash(&self) -> Byte32 {
        self.rtx.transaction.hash()
    }

    fn build_load_tx_hash(&self) -> LoadTxHash {
        LoadTxHash::new(self.hash())
    }

    fn build_load_cell(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadCell<'a> {
        LoadCell::new(
            &self.outputs,
            self.resolved_inputs(),
            self.resolved_cell_deps(),
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
            self.resolved_cell_deps(),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_input(&self, group_inputs: &'a [usize]) -> LoadInput {
        LoadInput::new(self.inputs(), group_inputs)
    }

    fn build_load_script_hash(&self, hash: Byte32) -> LoadScriptHash {
        LoadScriptHash::new(hash)
    }

    fn build_load_header(&'a self, group_inputs: &'a [usize]) -> LoadHeader<'a, DL> {
        LoadHeader::new(
            &self.data_loader,
            self.header_deps(),
            self.resolved_inputs(),
            self.resolved_cell_deps(),
            group_inputs,
        )
    }

    fn build_load_witness(&'a self, group_inputs: &'a [usize]) -> LoadWitness<'a> {
        LoadWitness::new(self.witnesses(), group_inputs)
    }

    // Extracts actual script binary either in dep cells.
    fn extract_script(&self, script: &'a Script) -> Result<Bytes, Error> {
        match script.hash_type().unpack() {
            ScriptHashType::Data => {
                if let Some(data) = self.binaries_by_data_hash.get(&script.code_hash()) {
                    Ok(data.to_owned())
                } else {
                    Err(ScriptError::InvalidCodeHash)?
                }
            }
            ScriptHashType::Type => {
                if let Some((data, multiple)) = self.binaries_by_type_hash.get(&script.code_hash())
                {
                    if *multiple {
                        Err(ScriptError::MultipleMatches)?
                    } else {
                        Ok(data.to_owned())
                    }
                } else {
                    Err(ScriptError::InvalidCodeHash)?
                }
            }
        }
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, Error> {
        let mut cycles: Cycle = 0;

        // Now run each script group
        for group in self.lock_groups.values().chain(self.type_groups.values()) {
            let cycle = self.verify_script_group(group, max_cycles).map_err(|e| {
                #[cfg(feature = "logging")]
                info!(
                    "Error validating script group {} of transaction {}: {:?}",
                    group.script.calc_script_hash(),
                    self.hash(),
                    e
                );
                e
            })?;
            let current_cycles = cycles
                .checked_add(cycle)
                .ok_or(ScriptError::ExceededMaximumCycles)?;
            if current_cycles > max_cycles {
                Err(ScriptError::ExceededMaximumCycles)?;
            }
            cycles = current_cycles;
        }
        Ok(cycles)
    }

    // Run a single script in current transaction, while this is not useful for
    // CKB itself, it can be very helpful when building a CKB debugger.
    pub fn verify_single(
        &self,
        script_group_type: &ScriptGroupType,
        script_hash: &Byte32,
        max_cycles: Cycle,
    ) -> Result<Cycle, Error> {
        let group = match script_group_type {
            ScriptGroupType::Lock => self.lock_groups.get(script_hash),
            ScriptGroupType::Type => self.type_groups.get(script_hash),
        };
        match group {
            Some(group) => self.verify_script_group(group, max_cycles),
            None => Err(ScriptError::InvalidCodeHash.into()),
        }
    }

    fn verify_script_group(&self, group: &ScriptGroup, max_cycles: Cycle) -> Result<Cycle, Error> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.pack()
            && group.script.hash_type().unpack() == ScriptHashType::Type
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
        }
    }

    fn run(
        &self,
        program: &Bytes,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, Error> {
        let current_script_hash = script_group.script.calc_script_hash();
        #[cfg(feature = "logging")]
        let prefix = format!("script group: {}", current_script_hash);
        let debug_printer = |message: &str| {
            if let Some(ref printer) = self.debug_printer {
                printer(&current_script_hash, message);
            } else {
                #[cfg(feature = "logging")]
                debug!("{} DEBUG OUTPUT: {}", prefix, message);
            };
        };
        let mut args = vec!["verify".into()];
        args.extend(
            script_group
                .script
                .args()
                .into_iter()
                .map(|arg| arg.raw_data()),
        );
        #[cfg(has_asm)]
        let machine_builder = {
            let core_machine = AsmCoreMachine::new_with_max_cycles(max_cycles);
            DefaultMachineBuilder::<Box<AsmCoreMachine>>::new(core_machine)
        };
        #[cfg(not(has_asm))]
        let machine_builder = {
            let core_machine =
                DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::new_with_max_cycles(
                    max_cycles,
                );
            DefaultMachineBuilder::<DefaultCoreMachine<u64, WXorXMemory<u64, SparseMemory<u64>>>>::new(core_machine)
        };
        let default_machine = machine_builder
            .instruction_cycle_func(Box::new(instruction_cycles))
            .syscall(Box::new(
                self.build_load_script_hash(current_script_hash.clone()),
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
        #[cfg(has_asm)]
        let mut machine = AsmMachine::new(default_machine, None);
        #[cfg(not(has_asm))]
        let mut machine = TraceMachine::new(default_machine);
        machine
            .load_program(&program, &args)
            .map_err(internal_error)?;
        let code = machine.run().map_err(internal_error)?;
        if code == 0 {
            Ok(machine.machine.cycles())
        } else {
            Err(ScriptError::ValidationFailure(code))?
        }
    }
}

fn internal_error(error: ckb_vm::Error) -> Error {
    InternalErrorKind::VM.cause(format!("{:?}", error)).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{ByteOrder, LittleEndian};
    use ckb_crypto::secp::{Generator, Privkey, Pubkey, Signature};
    use ckb_db::RocksDB;
    use ckb_hash::{blake2b_256, new_blake2b};
    use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainDB, COLUMNS};
    use ckb_types::{
        core::{
            capacity_bytes, cell::CellMetaBuilder, Capacity, ScriptHashType, TransactionBuilder,
            TransactionInfo,
        },
        h256,
        packed::{
            Byte32, CellDep, CellInput, CellOutputBuilder, OutPoint, Script,
            TransactionInfoBuilder, TransactionKeyBuilder,
        },
        H256,
    };
    use faster_hex::hex_encode;

    use ckb_error::assert_error_eq;
    use ckb_test_chain_utils::always_success_cell;
    use ckb_vm::Error as VMInternalError;
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
        ChainDB::new(RocksDB::open_tmp(COLUMNS), Default::default())
    }

    fn random_keypair() -> (Privkey, Pubkey) {
        Generator::random_keypair()
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

    fn default_transaction_info() -> TransactionInfo {
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

    #[test]
    fn check_always_success_hash() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![always_success_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);
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

        let code_hash = blake2b_256(&buffer);
        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point.clone())
            .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(code_hash.pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert!(verifier.verify(100_000_000).is_ok());

        // Not enough cycles
        assert_error_eq(
            verifier.verify(100).unwrap_err(),
            internal_error(VMInternalError::InvalidCycles),
        );
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

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .type_(
                Some(
                    Script::new_builder()
                        .code_hash(h256!("0x123456abcd90").pack())
                        .hash_type(ScriptHashType::Data.pack())
                        .build(),
                )
                .pack(),
            )
            .build();
        let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point.clone())
            .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(type_hash)
            .hash_type(ScriptHashType::Type.pack())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

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

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .type_(
                Some(
                    Script::new_builder()
                        .code_hash(h256!("0x123456abcd90").pack())
                        .hash_type(ScriptHashType::Data.pack())
                        .build(),
                )
                .pack(),
            )
            .build();
        let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data.clone())
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point.clone())
            .build();

        let dep_out_point2 = OutPoint::new(h256!("0x1234").pack(), 8);
        let cell_dep2 = CellDep::new_builder()
            .out_point(dep_out_point2.clone())
            .build();
        let output2 = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .type_(
                Some(
                    Script::new_builder()
                        .code_hash(h256!("0x123456abcd90").pack())
                        .hash_type(ScriptHashType::Data.pack())
                        .build(),
                )
                .pack(),
            )
            .build();
        let dep_cell2 = CellMetaBuilder::from_cell_output(output2, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point2.clone())
            .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(type_hash)
            .hash_type(ScriptHashType::Type.pack())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .cell_dep(cell_dep)
            .cell_dep(cell_dep2)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell, dep_cell2],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::MultipleMatches,
        );
    }

    #[test]
    fn check_signature_with_not_enough_cycles() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let privkey = Generator::random_privkey();
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

        let code_hash = blake2b_256(&buffer);
        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output.to_owned(), data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point.clone())
            .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(code_hash.pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

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

        let code_hash = blake2b_256(&buffer);
        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output.to_owned(), data)
            .transaction_info(default_transaction_info())
            .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(code_hash.pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::ValidationFailure(2),
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

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::InvalidCodeHash,
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
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let output_data = Bytes::default();
        let output = CellOutputBuilder::default()
            .lock(
                Script::new_builder()
                    .hash_type(ScriptHashType::Data.pack())
                    .build(),
            )
            .type_(Some(script).pack())
            .build();

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let dep_cell = {
            let data = Bytes::from(buffer);
            let output = CellOutputBuilder::default()
                .capacity(Capacity::bytes(data.len()).unwrap().pack())
                .build();
            CellMetaBuilder::from_cell_output(output.to_owned(), data)
                .transaction_info(default_transaction_info())
                .out_point(dep_out_point.clone())
                .build()
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .output_data(output_data.clone().pack())
            .cell_dep(cell_dep)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

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
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output.to_owned(), Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.to_owned(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();
        let output = CellOutputBuilder::default()
            .type_(Some(script).pack())
            .build();

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let dep_cell = {
            let dep_cell_data = Bytes::from(buffer);
            let output = CellOutputBuilder::default()
                .capacity(Capacity::bytes(dep_cell_data.len()).unwrap().pack())
                .build();
            CellMetaBuilder::from_cell_output(output, dep_cell_data)
                .transaction_info(default_transaction_info())
                .build()
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .output_data(Bytes::new().pack())
            .cell_dep(cell_dep)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::ValidationFailure(2),
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

        let script = Script::new_builder()
            .args(args.pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point.clone())
            .build();

        let transaction = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), 0))
            .cell_dep(cell_dep)
            .build();

        // The lock and type scripts here are both executed.
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script.clone())
            .type_(Some(script.clone()).pack())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        // Cycles can tell that both lock and type scripts are executed
        assert_eq!(verifier.verify(100_000_000).ok(), Some(2_818_104));
    }

    #[test]
    fn check_type_id_one_in_one_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(vec![Bytes::from(h256!("0x1111").as_ref())].pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.pack())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output().clone())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point.clone())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_one_in_one_out_not_enough_cycles() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(vec![Bytes::from(h256!("0x1111").as_ref())].pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.pack())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output().clone())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point.clone())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(500_000).unwrap_err(),
            ScriptError::ExceededMaximumCycles,
        );
    }

    #[test]
    fn check_type_id_creation() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(input.as_slice());
            blake2b.update(&0u64.to_le_bytes());
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            Bytes::from(&ret[..])
        };

        let type_id_script = Script::new_builder()
            .args(vec![input_hash].pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output().clone())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point.clone())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_termination() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(vec![Bytes::from(h256!("0x1111").as_ref())].pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.pack())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output().clone())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point.clone())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_invalid_creation() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(&input.previous_output().tx_hash().as_bytes());
            let mut buf = [0; 4];
            LittleEndian::write_u32(&mut buf, input.previous_output().index().unpack());
            blake2b.update(&buf[..]);
            let mut buf = [0; 8];
            LittleEndian::write_u64(&mut buf, 0);
            blake2b.update(&buf[..]);
            blake2b.update(b"unnecessary data");
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            Bytes::from(&ret[..])
        };

        let type_id_script = Script::new_builder()
            .args(vec![input_hash].pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output().clone())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point.clone())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(1_001_000).unwrap_err(),
            ScriptError::ValidationFailure(-3),
        );
    }

    #[test]
    fn check_type_id_invalid_creation_length() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(&input.previous_output().tx_hash().as_bytes());
            let mut buf = [0; 4];
            LittleEndian::write_u32(&mut buf, input.previous_output().index().unpack());
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

        let type_id_script = Script::new_builder()
            .args(vec![input_hash].pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output().clone())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point.clone())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(1_001_000).unwrap_err(),
            ScriptError::ValidationFailure(-1),
        );
    }

    #[test]
    fn check_type_id_one_in_two_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(vec![Bytes::from(h256!("0x1111").as_ref())].pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.pack())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(2000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();
        let output_cell2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .output(output_cell2)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output().clone())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point.clone())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        assert_error_eq(
            verifier.verify(1_001_000).unwrap_err(),
            ScriptError::ValidationFailure(-2),
        );
    }
}
