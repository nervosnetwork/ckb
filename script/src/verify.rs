use crate::{
    cost_model::instruction_cycles,
    syscalls::{Debugger, LoadCell, LoadHeader, LoadInput, LoadScriptHash, LoadTxHash},
    Runner, ScriptConfig, ScriptError,
};
use ckb_core::cell::{CellMeta, ResolvedOutPoint, ResolvedTransaction};
use ckb_core::extras::BlockExt;
use ckb_core::script::{Script, DAO_CODE_HASH};
use ckb_core::transaction::{CellInput, CellOutPoint};
use ckb_core::{BlockNumber, Capacity};
use ckb_core::{Bytes, Cycle};
use ckb_resource::bundled;
use ckb_store::{ChainStore, LazyLoadCellOutput};
use ckb_vm::{
    machine::asm::{AsmCoreMachine, AsmMachine},
    DefaultCoreMachine, DefaultMachineBuilder, SparseMemory, SupportMachine, TraceMachine,
};
use dao::calculate_maximum_withdraw;
use fnv::FnvHashMap;
use log::info;
use numext_fixed_hash::H256;
use std::cmp::min;
use std::path::PathBuf;
use std::sync::Arc;

pub const SYSTEM_DAO_CYCLES: u64 = 5000;

// TODO: tweak those values later
pub const DAO_LOCK_PERIOD_BLOCKS: BlockNumber = 10;
pub const DAO_MATURITY_BLOCKS: BlockNumber = 5;

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionScriptsVerifier<'a, CS> {
    store: Arc<CS>,
    binary_index: FnvHashMap<H256, usize>,
    block_data: FnvHashMap<H256, (BlockNumber, BlockExt)>,
    inputs: Vec<&'a CellInput>,
    outputs: Vec<CellMeta>,
    resolved_inputs: Vec<&'a ResolvedOutPoint>,
    resolved_deps: Vec<&'a ResolvedOutPoint>,
    witnesses: FnvHashMap<u32, &'a [Bytes]>,
    hash: H256,
    config: &'a ScriptConfig,
}

impl<'a, CS: ChainStore> TransactionScriptsVerifier<'a, CS> {
    pub fn new(
        rtx: &'a ResolvedTransaction,
        store: Arc<CS>,
        config: &'a ScriptConfig,
    ) -> TransactionScriptsVerifier<'a, CS> {
        let tx_hash = rtx.transaction.hash();
        let resolved_deps: Vec<&'a ResolvedOutPoint> = rtx.resolved_deps.iter().collect();
        let resolved_inputs: Vec<&'a ResolvedOutPoint> = rtx.resolved_inputs.iter().collect();
        let inputs: Vec<&'a CellInput> = rtx.transaction.inputs().iter().collect();
        let outputs = rtx
            .transaction
            .outputs()
            .iter()
            .enumerate()
            .map({
                |(index, output)| CellMeta {
                    cell_output: Some(output.clone()),
                    out_point: CellOutPoint {
                        tx_hash: tx_hash.to_owned(),
                        index: index as u32,
                    },
                    block_info: None,
                    cellbase: false,
                    capacity: output.capacity,
                    data_hash: None,
                }
            })
            .collect();
        let witnesses: FnvHashMap<u32, &'a [Bytes]> = rtx
            .transaction
            .witnesses()
            .iter()
            .enumerate()
            .map(|(idx, wit)| (idx as u32, &wit[..]))
            .collect();

        let binary_index: FnvHashMap<H256, usize> = resolved_deps
            .iter()
            .enumerate()
            .map(|(i, dep_cell)| {
                if let Some(cell_meta) = &dep_cell.cell.cell_meta() {
                    let hash = match cell_meta.data_hash() {
                        Some(hash) => hash.to_owned(),
                        None => {
                            let output = store.lazy_load_cell_output(cell_meta);
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

        let mut block_data = FnvHashMap::<H256, (BlockNumber, BlockExt)>::default();
        for resolved_input in &resolved_inputs {
            if let Some(header) = &resolved_input.header {
                if let Some(block_ext) = store.get_block_ext(header.hash()) {
                    block_data.insert(header.hash().to_owned(), (header.number(), block_ext));
                }
            }
        }
        for dep in &resolved_deps {
            if let Some(header) = &dep.header {
                if let Some(block_ext) = store.get_block_ext(header.hash()) {
                    block_data.insert(header.hash().to_owned(), (header.number(), block_ext));
                }
            }
        }

        TransactionScriptsVerifier {
            store,
            binary_index,
            block_data,
            inputs,
            outputs,
            resolved_inputs,
            resolved_deps,
            witnesses,
            config,
            hash: tx_hash.to_owned(),
        }
    }

    fn build_load_tx_hash(&self) -> LoadTxHash {
        LoadTxHash::new(&self.hash.as_bytes())
    }

    fn build_load_cell(&'a self) -> LoadCell<'a, CS> {
        LoadCell::new(
            Arc::clone(&self.store),
            &self.outputs,
            &self.resolved_inputs,
            &self.resolved_deps,
        )
    }

    fn build_load_input(&self) -> LoadInput {
        LoadInput::new(&self.inputs)
    }

    fn build_load_script_hash(&'a self, hash: &'a [u8]) -> LoadScriptHash<'a> {
        LoadScriptHash::new(hash)
    }

    fn build_load_header(&'a self) -> LoadHeader<'a> {
        LoadHeader::new(&self.resolved_inputs, &self.resolved_deps)
    }

    // Extracts actual script binary either in dep cells.
    fn extract_script(&self, script: &'a Script) -> Result<Bytes, ScriptError> {
        match self.binary_index.get(&script.code_hash).and_then(|index| {
            self.resolved_deps[*index]
                .cell
                .cell_meta()
                .map(|cell_meta| self.store.lazy_load_cell_output(&cell_meta))
        }) {
            Some(cell_output) => Ok(cell_output.data),
            None => Err(ScriptError::InvalidReferenceIndex),
        }
    }

    pub fn verify_script(
        &self,
        script: &Script,
        prefix: &str,
        appended_arguments: &[Bytes],
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        let current_script_hash = script.hash_with_appended_arguments(&appended_arguments);
        let mut args = vec!["verify".into()];
        args.extend_from_slice(&script.args);
        args.extend_from_slice(&appended_arguments);
        if script.code_hash == DAO_CODE_HASH {
            return self.verify_dao(&args, prefix, max_cycles, &current_script_hash.as_bytes());
        }
        self.extract_script(script).and_then(|script_binary| {
            self.run(
                &script_binary,
                &args,
                prefix,
                max_cycles,
                &current_script_hash.as_bytes(),
            )
        })
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let mut cycles = 0;
        for (i, (input, input_cell)) in self
            .inputs
            .iter()
            .zip(self.resolved_inputs.iter())
            .enumerate()
        {
            if input_cell.cell.is_issuing_dao_input() {
                if !self.valid_dao_withdraw_transaction() {
                    return Err(ScriptError::InvalidIssuingDaoInput);
                } else {
                    continue;
                }
            }
            let input_cell_meta = input_cell.cell.cell_meta();
            let input_cell = match &input_cell_meta {
                Some(cell) => cell,
                None => {
                    return Err(ScriptError::NoScript);
                }
            };
            let output = self.store.lazy_load_cell_output(input_cell);

            let prefix = format!("Transaction {:x}, input {}", self.hash, i);
            let mut appended_arguments = vec![];
            appended_arguments.extend_from_slice(&input.args);
            if let Some(witness) = self.witnesses.get(&(i as u32)) {
                appended_arguments.extend_from_slice(&witness);
            }

            let cycle = self.verify_script(&output.lock, &prefix, &appended_arguments, max_cycles - cycles).map_err(|e| {
                info!(target: "script", "Error validating input {} of transaction {:x}: {:?}", i, self.hash, e);
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
        for (i, cell_meta) in self.outputs.iter().enumerate() {
            let output = cell_meta.cell_output.as_ref().expect("output already set");
            if let Some(ref type_) = output.type_ {
                let prefix = format!("Transaction {:x}, output {}", self.hash, i);
                let cycle = self.verify_script(type_, &prefix, &[], max_cycles - cycles).map_err(|e| {
                    info!(target: "script", "Error validating output {} of transaction {:x}: {:?}", i, self.hash, e);
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
        }
        Ok(cycles)
    }

    fn verify_dao(
        &self,
        args: &[Bytes],
        prefix: &str,
        max_cycles: Cycle,
        current_script_hash: &[u8],
    ) -> Result<Cycle, ScriptError> {
        if args.len() != 6 {
            return Err(ScriptError::ArgumentNumber);
        }
        // DAO accepts 6 arguments in the following format:
        // 0. program name
        // 1. pubkey hash(20 bytes) from lock script args
        // 2. withdraw block hash(32 bytes) from input args
        // 3. pubkey(33 bytes) from witness
        // 4. signature from witness
        // 5. size of signature field from witness, the actual value is stored in little endian formatted 64-bit unsigned integer
        // Note argument 2 is not required in the default lock, hence we are processing
        // the list a bit here.
        let lock_arguments = vec![
            args[0].to_owned(),
            args[1].to_owned(),
            args[3].to_owned(),
            args[4].to_owned(),
            args[5].to_owned(),
        ];
        let cycles = self
            .verify_default_lock(&lock_arguments, prefix, max_cycles, current_script_hash)?
            .checked_add(SYSTEM_DAO_CYCLES)
            .ok_or(ScriptError::ExceededMaximumCycles)?;;
        if cycles > max_cycles {
            return Err(ScriptError::ExceededMaximumCycles);
        }

        let withdraw_header_hash = H256::from_slice(&args[2]).map_err(|_| ScriptError::IOError)?;
        let (withdraw_block_number, withdraw_block_ext) = self
            .block_data
            .get(&withdraw_header_hash)
            .ok_or(ScriptError::InvalidDaoWithdrawHeader)?;

        let mut maximum_output_capacities = Capacity::zero();
        for (input, resolved_input) in self.inputs.iter().zip(self.resolved_inputs.iter()) {
            if resolved_input.cell().is_none() {
                continue;
            }
            let cell_meta = resolved_input.cell().unwrap();
            let output = self.store.lazy_load_cell_output(&cell_meta);
            if output.lock.code_hash != DAO_CODE_HASH {
                continue;
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
        args: &[Bytes],
        prefix: &str,
        max_cycles: Cycle,
        current_script_hash: &[u8],
    ) -> Result<Cycle, ScriptError> {
        // TODO: this is a temporary solution for now, we can change this to use
        // composable contracts when we manage to build NervosDAO as a script
        // running on CKB VM.
        let program = bundled(PathBuf::from("specs/cells/secp256k1_blake160_sighash_all"))
            .ok_or(ScriptError::NoScript)?
            .get()
            .map_err(|_| ScriptError::IOError)?;

        self.run(
            &program.into_owned().into(),
            args,
            prefix,
            max_cycles,
            current_script_hash,
        )
    }

    fn run(
        &self,
        program: &Bytes,
        args: &[Bytes],
        prefix: &str,
        max_cycles: Cycle,
        current_script_hash: &[u8],
    ) -> Result<Cycle, ScriptError> {
        let (code, cycles) = match self.config.runner {
            Runner::Assembly => {
                let core_machine = AsmCoreMachine::new_with_max_cycles(max_cycles);
                let machine = DefaultMachineBuilder::<Box<AsmCoreMachine>>::new(core_machine)
                    .instruction_cycle_func(Box::new(instruction_cycles))
                    .syscall(Box::new(self.build_load_script_hash(current_script_hash)))
                    .syscall(Box::new(self.build_load_tx_hash()))
                    .syscall(Box::new(self.build_load_cell()))
                    .syscall(Box::new(self.build_load_input()))
                    .syscall(Box::new(self.build_load_header()))
                    .syscall(Box::new(Debugger::new(prefix)))
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
                    DefaultCoreMachine::<u64, SparseMemory<u64>>::new_with_max_cycles(max_cycles);
                let machine =
                    DefaultMachineBuilder::<DefaultCoreMachine<u64, SparseMemory<u64>>>::new(
                        core_machine,
                    )
                    .instruction_cycle_func(Box::new(instruction_cycles))
                    .syscall(Box::new(self.build_load_script_hash(current_script_hash)))
                    .syscall(Box::new(self.build_load_tx_hash()))
                    .syscall(Box::new(self.build_load_cell()))
                    .syscall(Box::new(self.build_load_input()))
                    .syscall(Box::new(self.build_load_header()))
                    .syscall(Box::new(Debugger::new(prefix)))
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

    fn valid_dao_withdraw_transaction(&self) -> bool {
        self.resolved_inputs.iter().any(|input| {
            input
                .cell
                .cell_meta()
                .map(|cell| {
                    let output = self.store.lazy_load_cell_output(&cell);
                    output.lock.code_hash == DAO_CODE_HASH
                })
                .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{LittleEndian, WriteBytesExt};
    use ckb_core::cell::CellMetaBuilder;
    use ckb_core::extras::DaoStats;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::{capacity_bytes, Capacity};
    use ckb_db::MemoryKeyValueDB;
    use ckb_store::{ChainKVStore, StoreBatch, COLUMNS};
    use crypto::secp::Generator;
    use faster_hex::hex_encode;
    use hash::{blake2b_256, sha3_256};
    use numext_fixed_hash::{h256, H256};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::sync::Arc;
    use test_chain_utils::create_always_success_cell;

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
            always_success_script,
            None,
        );
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_number(1)
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(always_success_cell)
                .block_number(1)
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![always_success_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

        assert!(verifier.verify(100).is_ok());
    }

    #[test]
    fn check_signature() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        witness_data.insert(0, Bytes::from(hex_signature));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, Bytes::from(hex_pubkey));

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
                .block_number(1)
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_number(1)
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_rust() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        witness_data.insert(0, Bytes::from(hex_signature));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, Bytes::from(hex_pubkey));

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
                .block_number(1)
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.clone().unwrap())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_number(1)
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };
        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Rust,
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
        let args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        witness_data.insert(0, Bytes::from(hex_signature));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, Bytes::from(hex_pubkey));

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
                .block_number(1)
                .data_hash(code_hash.to_owned())
                .out_point(dep_out_point.cell.as_ref().unwrap().clone())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_number(1)
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        witness_data.insert(0, Bytes::from(hex_signature));
        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, Bytes::from(hex_pubkey));

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
                .block_number(1)
                .data_hash(code_hash.to_owned())
                .build(),
        );

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_number(1)
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        witness_data.insert(0, Bytes::from(hex_signature));

        let dep_out_point = OutPoint::new_cell(h256!("0x123"), 8);

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, Bytes::from(hex_pubkey));

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output)
                .block_number(1)
                .build(),
        );

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            resolved_deps: vec![],
            resolved_inputs: vec![dummy_cell],
        };

        let store = Arc::new(new_memory_store());
        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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

        let input = CellInput::new(OutPoint::null(), 0, vec![]);
        let (always_success_cell, always_success_script) = create_always_success_cell();
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            always_success_script,
            None,
        );
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_number(1)
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(always_success_cell)
                .block_number(1)
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
                    .block_number(1)
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
        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, Bytes::from(hex_signature));
        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.insert(0, Bytes::from(hex_pubkey));

        let input = CellInput::new(OutPoint::null(), 0, vec![]);
        let (always_success_cell, always_success_script) = create_always_success_cell();
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            always_success_script,
            None,
        );
        let dummy_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(output.to_owned())
                .block_number(1)
                .build(),
        );
        let always_success_cell = ResolvedOutPoint::cell_only(
            CellMetaBuilder::from_cell_output(always_success_cell.to_owned())
                .block_number(1)
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
                    .block_number(1)
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_tx_with_only_dao_issuing_input_but_no_dao_input() {
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
            vec![withdraw_header.hash().as_bytes().into()],
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
        let signature = privkey.sign_recoverable(&transaction_hash).unwrap();
        let signature_der = signature.serialize_der();
        let mut signature_size = vec![];
        signature_size
            .write_u64::<LittleEndian>(signature_der.len() as u64)
            .unwrap();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(signature_size),
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
            vec![withdraw_header.hash().as_bytes().into()],
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
        let signature = privkey.sign_recoverable(&transaction_hash).unwrap();
        let signature_der = signature.serialize_der();
        let mut signature_size = vec![];
        signature_size
            .write_u64::<LittleEndian>(signature_der.len() as u64)
            .unwrap();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(signature_size),
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
            vec![withdraw_header.hash().as_bytes().into()],
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
        let mut signature_size = vec![];
        signature_size
            .write_u64::<LittleEndian>(signature_der.len() as u64)
            .unwrap();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(signature_size),
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
            vec![withdraw_header.hash().as_bytes().into()],
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
        let signature = privkey.sign_recoverable(&transaction_hash).unwrap();
        let signature_der = signature.serialize_der();
        let mut signature_size = vec![];
        signature_size
            .write_u64::<LittleEndian>(signature_der.len() as u64)
            .unwrap();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(signature_size),
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
            vec![withdraw_header.hash().as_bytes().into()],
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
        let signature = privkey.sign_recoverable(&transaction_hash).unwrap();
        let signature_der = signature.serialize_der();
        let mut signature_size = vec![];
        signature_size
            .write_u64::<LittleEndian>(signature_der.len() as u64)
            .unwrap();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(signature_size),
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1061,
            vec![withdraw_header.hash().as_bytes().into()],
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
        let signature = privkey.sign_recoverable(&transaction_hash).unwrap();
        let signature_der = signature.serialize_der();
        let mut signature_size = vec![];
        signature_size
            .write_u64::<LittleEndian>(signature_der.len() as u64)
            .unwrap();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(signature_size),
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

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
        let input = CellInput::new(OutPoint::new_issuing_dao(), 0, vec![]);
        let input2 = CellInput::new(
            OutPoint::new(deposit_header.hash().to_owned(), h256!("0x3"), 0),
            1060,
            vec![withdraw_header.hash().as_bytes().into()],
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
        let signature = privkey.sign_recoverable(&transaction_hash).unwrap();
        let signature_der = signature.serialize_der();
        let mut signature_size = vec![];
        signature_size
            .write_u64::<LittleEndian>(signature_der.len() as u64)
            .unwrap();

        let transaction = TransactionBuilder::default()
            .input(input)
            .input(input2)
            .output(withdraw_output)
            .dep(OutPoint::new_block_hash(withdraw_header.hash().to_owned()))
            .witness(vec![])
            .witness(vec![
                Bytes::from(pubkey),
                Bytes::from(signature_der),
                Bytes::from(signature_size),
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

        let verifier = TransactionScriptsVerifier::new(
            &rtx,
            store,
            &ScriptConfig {
                runner: Runner::Assembly,
            },
        );

        assert!(verifier.verify(100_000_000).is_err());
    }
}
