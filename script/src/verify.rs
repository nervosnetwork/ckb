use crate::{
    cost_model::{instruction_cycles, transferred_byte_cycles},
    error::ScriptError,
    syscalls::{
        Debugger, LoadCell, LoadCellData, LoadHeader, LoadInput, LoadScript, LoadScriptHash,
        LoadTx, LoadWitness,
    },
    type_id::TypeIdSystemScript,
    types::{ScriptGroup, ScriptGroupType},
};
use ckb_chain_spec::consensus::TYPE_ID_CODE_HASH;
use ckb_error::Error;
#[cfg(feature = "logging")]
use ckb_logger::{debug, info};
use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::{CellMeta, ResolvedTransaction},
        Cycle, ScriptHashType,
    },
    packed::{Byte32, Byte32Vec, BytesVec, CellInputVec, CellOutput, OutPoint, Script},
    prelude::*,
};
#[cfg(has_asm)]
use ckb_vm::{
    machine::asm::{AsmCoreMachine, AsmMachine},
    DefaultMachineBuilder, Error as VMInternalError, InstructionCycleFunc, SupportMachine,
    Syscalls,
};
#[cfg(not(has_asm))]
use ckb_vm::{
    DefaultCoreMachine, DefaultMachineBuilder, Error as VMInternalError, InstructionCycleFunc,
    SparseMemory, SupportMachine, Syscalls, TraceMachine, WXorXMemory,
};
use std::collections::HashMap;
use std::convert::TryFrom;

#[cfg(has_asm)]
type CoreMachineType = Box<AsmCoreMachine>;
#[cfg(not(has_asm))]
type CoreMachineType = DefaultCoreMachine<u64, WXorXMemory<u64, SparseMemory<u64>>>;

/// This struct leverages CKB VM to verify transaction inputs.
///
/// FlatBufferBuilder owned `Vec<u8>` that grows as needed, in the
/// future, we might refactor this to share buffer to achieve zero-copy
pub struct TransactionScriptsVerifier<'a, DL> {
    data_loader: &'a DL,
    debug_printer: Box<dyn Fn(&Byte32, &str)>,

    outputs: Vec<CellMeta>,
    rtx: &'a ResolvedTransaction,

    binaries_by_data_hash: HashMap<Byte32, Bytes>,
    binaries_by_type_hash: HashMap<Byte32, (Bytes, bool)>,
    lock_groups: HashMap<Byte32, ScriptGroup>,
    type_groups: HashMap<Byte32, ScriptGroup>,
}

impl<'a, DL: CellDataProvider + HeaderProvider> TransactionScriptsVerifier<'a, DL> {
    /// TODO(doc): @doitian
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
                .or_insert_with(|| ScriptGroup::from_lock_script(&output.lock()));
            lock_group_entry.input_indices.push(i);
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(&t));
                type_group_entry.input_indices.push(i);
            }
        }
        for (i, output) in rtx.transaction.outputs().into_iter().enumerate() {
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(&t));
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
            debug_printer: Box::new(
                #[allow(unused_variables)]
                |hash: &Byte32, message: &str| {
                    #[cfg(feature = "logging")]
                    debug!("script group: {} DEBUG OUTPUT: {}", hash, message);
                },
            ),
        }
    }

    /// TODO(doc): @doitian
    pub fn set_debug_printer<F: Fn(&Byte32, &str) + 'static>(&mut self, func: F) {
        self.debug_printer = Box::new(func);
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
    fn witnesses(&self) -> BytesVec {
        self.rtx.transaction.witnesses()
    }

    #[inline]
    #[allow(dead_code)]
    fn hash(&self) -> Byte32 {
        self.rtx.transaction.hash()
    }

    fn build_load_tx(&self) -> LoadTx {
        LoadTx::new(&self.rtx.transaction)
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

    fn build_load_witness(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadWitness<'a> {
        LoadWitness::new(self.witnesses(), group_inputs, group_outputs)
    }

    fn build_load_script(&self, script: Script) -> LoadScript {
        LoadScript::new(script)
    }

    /// Extracts actual script binary either in dep cells.
    pub fn extract_script(&self, script: &'a Script) -> Result<Bytes, ScriptError> {
        match ScriptHashType::try_from(script.hash_type()).expect("checked data") {
            ScriptHashType::Data => {
                if let Some(data) = self.binaries_by_data_hash.get(&script.code_hash()) {
                    Ok(data.to_owned())
                } else {
                    Err(ScriptError::InvalidCodeHash)
                }
            }
            ScriptHashType::Type => {
                if let Some((data, multiple)) = self.binaries_by_type_hash.get(&script.code_hash())
                {
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

    /// TODO(doc): @doitian
    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, Error> {
        let mut cycles: Cycle = 0;

        // Now run each script group
        for group in self.lock_groups.values().chain(self.type_groups.values()) {
            let cycle = self.verify_script_group(group, max_cycles).map_err(|e| {
                #[cfg(feature = "logging")]
                info!(
                    "Error validating script group {} of transaction {}: {}",
                    group.script.calc_script_hash(),
                    self.hash(),
                    e
                );
                e.source(group)
            })?;
            let current_cycles = cycles
                .checked_add(cycle)
                .ok_or_else(|| ScriptError::ExceededMaximumCycles(max_cycles).source(group))?;
            if current_cycles > max_cycles {
                return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                    .source(group)
                    .into());
            }
            cycles = current_cycles;
        }
        Ok(cycles)
    }

    /// Runs a single script in current transaction, while this is not useful for
    /// CKB itself, it can be very helpful when building a CKB debugger.
    pub fn verify_single(
        &self,
        script_group_type: ScriptGroupType,
        script_hash: &Byte32,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        match self.find_script_group(script_group_type, script_hash) {
            Some(group) => self.verify_script_group(group, max_cycles),
            None => Err(ScriptError::InvalidCodeHash),
        }
    }

    fn verify_script_group(
        &self,
        group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.pack()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: self.rtx,
                script_group: group,
                max_cycles,
            };
            verifier.verify()
        } else {
            self.run(&group, max_cycles)
        }
    }

    /// TODO(doc): @doitian
    pub fn find_script_group(
        &self,
        script_group_type: ScriptGroupType,
        script_hash: &Byte32,
    ) -> Option<&ScriptGroup> {
        match script_group_type {
            ScriptGroupType::Lock => self.lock_groups.get(script_hash),
            ScriptGroupType::Type => self.type_groups.get(script_hash),
        }
    }

    /// TODO(doc): @doitian
    pub fn cost_model(&self) -> Box<InstructionCycleFunc> {
        Box::new(instruction_cycles)
    }

    /// TODO(doc): @doitian
    pub fn generate_syscalls(
        &'a self,
        script_group: &'a ScriptGroup,
    ) -> Vec<Box<(dyn Syscalls<CoreMachineType> + 'a)>> {
        let current_script_hash = script_group.script.calc_script_hash();
        vec![
            Box::new(self.build_load_script_hash(current_script_hash.clone())),
            Box::new(self.build_load_tx()),
            Box::new(
                self.build_load_cell(&script_group.input_indices, &script_group.output_indices),
            ),
            Box::new(self.build_load_input(&script_group.input_indices)),
            Box::new(self.build_load_header(&script_group.input_indices)),
            Box::new(
                self.build_load_witness(&script_group.input_indices, &script_group.output_indices),
            ),
            Box::new(self.build_load_script(script_group.script.clone())),
            Box::new(
                self.build_load_cell_data(
                    &script_group.input_indices,
                    &script_group.output_indices,
                ),
            ),
            Box::new(Debugger::new(current_script_hash, &self.debug_printer)),
        ]
    }

    fn run(&self, script_group: &ScriptGroup, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let program = self.extract_script(&script_group.script)?;
        #[cfg(has_asm)]
        let core_machine = AsmCoreMachine::new_with_max_cycles(max_cycles);
        #[cfg(not(has_asm))]
        let core_machine =
            DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::new_with_max_cycles(
                max_cycles,
            );
        let machine_builder = DefaultMachineBuilder::<CoreMachineType>::new(core_machine)
            .instruction_cycle_func(self.cost_model());
        let machine_builder = self
            .generate_syscalls(script_group)
            .into_iter()
            .fold(machine_builder, |builder, syscall| builder.syscall(syscall));
        let default_machine = machine_builder.build();
        #[cfg(has_asm)]
        let mut machine = AsmMachine::new(default_machine, None);
        #[cfg(not(has_asm))]
        let mut machine = TraceMachine::new(default_machine);

        let map_vm_internal_error = |error: VMInternalError| match error {
            VMInternalError::InvalidCycles => ScriptError::ExceededMaximumCycles(max_cycles),
            _ => ScriptError::VMInternalError(format!("{:?}", error)),
        };

        let bytes = machine
            .load_program(&program, &[])
            .map_err(map_vm_internal_error)?;
        machine
            .machine
            .add_cycles(transferred_byte_cycles(bytes))
            .map_err(map_vm_internal_error)?;
        let code = machine.run().map_err(map_vm_internal_error)?;
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
    use crate::type_id::TYPE_ID_CYCLES;
    use byteorder::{ByteOrder, LittleEndian};
    use ckb_crypto::secp::{Generator, Privkey, Pubkey, Signature};
    use ckb_db::RocksDB;
    use ckb_hash::{blake2b_256, new_blake2b};
    use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainDB, COLUMNS};
    use ckb_types::{
        core::{
            capacity_bytes, cell::CellMetaBuilder, Capacity, Cycle, DepType, ScriptHashType,
            TransactionBuilder, TransactionInfo,
        },
        h256,
        packed::{
            Byte32, CellDep, CellInput, CellOutputBuilder, OutPoint, Script,
            TransactionInfoBuilder, TransactionKeyBuilder, WitnessArgs,
        },
        H256,
    };
    use faster_hex::hex_encode;

    use ckb_chain_spec::consensus::{TWO_IN_TWO_OUT_BYTES, TWO_IN_TWO_OUT_CYCLES};
    use ckb_error::assert_error_eq;
    use ckb_test_chain_utils::{
        always_success_cell, ckb_testnet_consensus, secp256k1_blake160_sighash_cell,
        secp256k1_data_cell, type_lock_script_code_hash,
    };
    use std::fs::File;
    use std::io::Read;
    use std::path::Path;

    const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;
    const CYCLE_BOUND: Cycle = 200_000;

    fn sha3_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
        tiny_keccak::sha3_256(s.as_ref())
    }

    // NOTE: `verify` binary is outdated and most related unit tests are testing `script` crate functions
    // I try to keep unit test code unmodified as much as possible, and may add it back in future PR.
    // fn open_cell_verify() -> File {
    //     File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap()
    // }

    fn open_cell_always_success() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/always_success"))
            .unwrap()
    }

    fn open_cell_always_failure() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/always_failure"))
            .unwrap()
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

    fn sign_args(args: &[u8], privkey: &Privkey) -> Signature {
        let hash = sha3_256(sha3_256(args));
        privkey.sign_recoverable(&hash.into()).unwrap()
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

        let transaction = TransactionBuilder::default().input(input).build();

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
        assert!(verifier.verify(600).is_ok());
    }

    #[test]
    fn check_signature() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

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
            .out_point(dep_out_point)
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(code_hash.pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
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
        assert_error_eq!(
            verifier
                .verify(ALWAYS_SUCCESS_SCRIPT_CYCLE - 1)
                .unwrap_err(),
            ScriptError::ExceededMaximumCycles(ALWAYS_SUCCESS_SCRIPT_CYCLE - 1)
                .input_lock_script(0),
        );

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_referenced_via_type_hash() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

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
                        .hash_type(ScriptHashType::Data.into())
                        .build(),
                )
                .pack(),
            )
            .build();
        let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point)
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(type_hash)
            .hash_type(ScriptHashType::Type.into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
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
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();
        let data = Bytes::from(buffer);

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

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
                        .hash_type(ScriptHashType::Data.into())
                        .build(),
                )
                .pack(),
            )
            .build();
        let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data.clone())
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point)
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
                        .hash_type(ScriptHashType::Data.into())
                        .build(),
                )
                .pack(),
            )
            .build();
        let dep_cell2 = CellMetaBuilder::from_cell_output(output2, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point2)
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(type_hash)
            .hash_type(ScriptHashType::Type.into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
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

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::MultipleMatches.input_lock_script(0),
        );
    }

    #[test]
    fn check_invalid_signature() {
        let mut file = open_cell_always_failure();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);

        // This line makes the verification invalid
        args.extend(&b"extrastring".to_vec());
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let code_hash = blake2b_256(&buffer);
        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(code_hash.pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
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

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::ValidationFailure(-1).input_lock_script(0),
        );
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();
        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
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

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::InvalidCodeHash.input_lock_script(0),
        );
    }

    #[test]
    fn check_output_contract() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();
        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let output_data = Bytes::default();
        let output = CellOutputBuilder::default()
            .lock(
                Script::new_builder()
                    .hash_type(ScriptHashType::Data.into())
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
            CellMetaBuilder::from_cell_output(output, data)
                .transaction_info(default_transaction_info())
                .out_point(dep_out_point)
                .build()
        };

        let transaction = TransactionBuilder::default()
            .input(input)
            .output(output)
            .output_data(output_data.pack())
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
        let mut file = open_cell_always_failure();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        // This line makes the verification invalid
        args.extend(&b"extrastring".to_vec());
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.to_owned(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let output = CellOutputBuilder::default()
            .type_(Some(script).pack())
            .build();

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();
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
            .input(input)
            .output(output)
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

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::ValidationFailure(-1).output_type_script(0),
        );
    }

    #[test]
    fn check_same_lock_and_type_script_are_executed_twice() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let privkey = Privkey::from_slice(&[1; 32][..]);
        let pubkey = privkey.pubkey().unwrap();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data.into())
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
            .out_point(dep_out_point)
            .build();

        let transaction = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), 0))
            .cell_dep(cell_dep)
            .build();

        // The lock and type scripts here are both executed.
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script.clone())
            .type_(Some(script).pack())
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
        assert_eq!(
            verifier.verify(100_000_000).ok(),
            Some(ALWAYS_SUCCESS_SCRIPT_CYCLE * 2)
        );
    }

    #[test]
    fn check_type_id_one_in_one_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
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
            .type_(Some(type_id_script).pack())
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
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
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

        if let Err(err) = verifier.verify(TYPE_ID_CYCLES * 2) {
            panic!("expect verification ok, got: {:?}", err);
        }
    }

    #[test]
    fn check_type_id_one_in_one_out_not_enough_cycles() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
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
            .type_(Some(type_id_script).pack())
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
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
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

        assert_error_eq!(
            verifier.verify(TYPE_ID_CYCLES - 1).unwrap_err(),
            ScriptError::ExceededMaximumCycles(TYPE_ID_CYCLES - 1).input_type_script(0),
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
            Bytes::from(ret.to_vec())
        };

        let type_id_script = Script::new_builder()
            .args(input_hash.pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
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
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
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
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
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
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
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
            Bytes::from(ret.to_vec())
        };

        let type_id_script = Script::new_builder()
            .args(input_hash.pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
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
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
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

        assert_error_eq!(
            verifier.verify(1_001_000).unwrap_err(),
            ScriptError::ValidationFailure(-3).output_type_script(0),
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
            Bytes::from(buf)
        };

        let type_id_script = Script::new_builder()
            .args(input_hash.pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
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
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
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

        assert_error_eq!(
            verifier.verify(1_001_000).unwrap_err(),
            ScriptError::ValidationFailure(-1).output_type_script(0),
        );
    }

    #[test]
    fn check_type_id_one_in_two_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
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
            .type_(Some(type_id_script).pack())
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
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
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

        assert_error_eq!(
            verifier.verify(TYPE_ID_CYCLES * 2).unwrap_err(),
            ScriptError::ValidationFailure(-2).input_type_script(0),
        );
    }

    #[test]
    fn check_typical_secp256k1_blake160_2_in_2_out_tx() {
        let consensus = ckb_testnet_consensus();
        let dep_group_tx_hash = consensus.genesis_block().transactions()[1].hash();
        let secp_out_point = OutPoint::new(dep_group_tx_hash, 0);

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.into())
            .build();

        let input1 = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 0), 0);
        let input2 = CellInput::new(OutPoint::new(h256!("0x1111").pack(), 0), 0);

        let mut generator = Generator::non_crypto_safe_prng(42);
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from((&blake2b_256(&pubkey_data)[0..20]).to_owned());
        let privkey2 = generator.gen_privkey();
        let pubkey_data2 = privkey2.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg2 = Bytes::from((&blake2b_256(&pubkey_data2)[0..20]).to_owned());

        let lock = Script::new_builder()
            .args(lock_arg.pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let lock2 = Script::new_builder()
            .args(lock_arg2.pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output1 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock.clone())
            .build();
        let output2 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock2.clone())
            .build();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep)
            .input(input1.clone())
            .input(input2.clone())
            .output(output1)
            .output(output2)
            .output_data(Default::default())
            .output_data(Default::default())
            .build();

        let tx_hash: H256 = tx.hash().unpack();
        // sign input1
        let witness = {
            WitnessArgs::new_builder()
                .lock(Some(Bytes::from(vec![0u8; 65])).pack())
                .build()
        };
        let witness_len: u64 = witness.as_bytes().len() as u64;
        let mut hasher = new_blake2b();
        hasher.update(tx_hash.as_bytes());
        hasher.update(&witness_len.to_le_bytes());
        hasher.update(&witness.as_bytes());
        let message = {
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        let sig = privkey.sign_recoverable(&message).expect("sign");
        let witness = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(sig.serialize())).pack())
            .build();
        // sign input2
        let witness2 = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(vec![0u8; 65])).pack())
            .build();
        let witness2_len: u64 = witness2.as_bytes().len() as u64;
        let mut hasher = new_blake2b();
        hasher.update(tx_hash.as_bytes());
        hasher.update(&witness2_len.to_le_bytes());
        hasher.update(&witness2.as_bytes());
        let message2 = {
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        let sig2 = privkey2.sign_recoverable(&message2).expect("sign");
        let witness2 = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(sig2.serialize())).pack())
            .build();
        let tx = tx
            .as_advanced_builder()
            .witness(witness.as_bytes().pack())
            .witness(witness2.as_bytes().pack())
            .build();

        let serialized_size = tx.data().as_slice().len() as u64;

        assert_eq!(
            serialized_size, TWO_IN_TWO_OUT_BYTES,
            "2 in 2 out tx serialized size changed, PLEASE UPDATE consensus"
        );

        let (secp256k1_blake160_cell, secp256k1_blake160_cell_data) =
            secp256k1_blake160_sighash_cell(consensus.clone());

        let (secp256k1_data_cell, secp256k1_data_cell_data) = secp256k1_data_cell(consensus);

        let input_cell1 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock)
            .build();

        let resolved_input_cell1 =
            CellMetaBuilder::from_cell_output(input_cell1, Default::default())
                .out_point(input1.previous_output())
                .build();

        let input_cell2 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock2)
            .build();

        let resolved_input_cell2 =
            CellMetaBuilder::from_cell_output(input_cell2, Default::default())
                .out_point(input2.previous_output())
                .build();

        let resolved_secp256k1_blake160_cell = CellMetaBuilder::from_cell_output(
            secp256k1_blake160_cell,
            secp256k1_blake160_cell_data,
        )
        .build();

        let resolved_secp_data_cell =
            CellMetaBuilder::from_cell_output(secp256k1_data_cell, secp256k1_data_cell_data)
                .build();

        let rtx = ResolvedTransaction {
            transaction: tx,
            resolved_cell_deps: vec![resolved_secp256k1_blake160_cell, resolved_secp_data_cell],
            resolved_inputs: vec![resolved_input_cell1, resolved_input_cell2],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);

        let verifier = TransactionScriptsVerifier::new(&rtx, &data_loader);

        let cycle = verifier.verify(TWO_IN_TWO_OUT_CYCLES).unwrap();
        assert!(cycle <= TWO_IN_TWO_OUT_CYCLES);
        assert!(cycle >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
    }
}
