use crate::types::CoreMachine as ICoreMachine;
use crate::v2_syscalls::INDEX_OUT_OF_BOUND;
use crate::v2_types::PipeIoArgs;
use crate::verify::TransactionScriptsSyscallsGenerator;
use crate::{
    v2_syscalls::{
        transferred_byte_cycles, MachineContext, INVALID_PIPE, OTHER_END_CLOSED, SUCCESS,
        WAIT_FAILURE,
    },
    v2_types::{
        DataPieceId, FullSuspendedState, Message, PipeId, RunMode, TxData, VmId, VmState,
        FIRST_PIPE_SLOT, FIRST_VM_ID,
    },
};
use crate::{ScriptVersion, TransactionScriptsVerifier};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::core::Cycle;
use ckb_vm::{
    bytes::Bytes,
    cost_model::estimate_cycles,
    elf::parse_elf,
    machine::{
        asm::{AsmCoreMachine, AsmMachine},
        CoreMachine, DefaultMachineBuilder, Pause, SupportMachine,
    },
    memory::Memory,
    registers::A0,
    snapshot2::{DataSource, Snapshot2},
    Error, Register, Syscalls,
};
use std::sync::{Arc, Mutex};
use std::{
    collections::{BTreeMap, HashMap},
    mem::size_of,
};

const ROOT_VM_ID: VmId = FIRST_VM_ID;
const MAX_INSTANTIATED_VMS: usize = 4;

/// A single Scheduler instance is used to verify a single script
/// within a CKB transaction.
///
/// A scheduler holds & manipulates a core, the scheduler also holds
/// all CKB-VM machines, each CKB-VM machine also gets a mutable reference
/// of the core for IO operations.
pub struct Scheduler<
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
> {
    tx_data: TxData<DL>,
    // In fact, Scheduler here has the potential to totally replace
    // TransactionScriptsVerifier, nonetheless much of current syscall
    // implementation is strictly tied to TransactionScriptsVerifier, we
    // are using it here to save some extra code.
    script_version: ScriptVersion,
    syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,

    total_cycles: Cycle,
    next_vm_id: VmId,
    next_pipe_slot: u64,
    states: BTreeMap<VmId, VmState>,
    pipes: HashMap<PipeId, VmId>,
    inherited_fd: BTreeMap<VmId, Vec<PipeId>>,
    instantiated: BTreeMap<VmId, (MachineContext<DL>, AsmMachine)>,
    suspended: HashMap<VmId, Snapshot2<DataPieceId>>,
    terminated_vms: HashMap<VmId, i8>,

    // message_box is expected to be empty before returning from `run`
    // function, there is no need to persist messages.
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl<DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static>
    Scheduler<DL>
{
    /// Create a new scheduler from empty state
    pub fn new(
        tx_data: TxData<DL>,
        script_version: ScriptVersion,
        syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,
    ) -> Self {
        let message_box = syscalls_generator.message_box.clone();
        Self {
            tx_data,
            script_version,
            syscalls_generator,
            total_cycles: 0,
            next_vm_id: FIRST_VM_ID,
            next_pipe_slot: FIRST_PIPE_SLOT,
            states: BTreeMap::default(),
            pipes: HashMap::default(),
            inherited_fd: BTreeMap::default(),
            instantiated: BTreeMap::default(),
            suspended: HashMap::default(),
            message_box,
            terminated_vms: HashMap::default(),
        }
    }

    pub fn consumed_cycles(&self) -> Cycle {
        self.total_cycles
    }

    /// Resume a previously suspended scheduler state
    pub fn resume(
        tx_data: TxData<DL>,
        script_version: ScriptVersion,
        syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,
        full: FullSuspendedState,
    ) -> Self {
        Self {
            tx_data,
            script_version,
            syscalls_generator,
            total_cycles: full.total_cycles,
            next_vm_id: full.next_vm_id,
            next_pipe_slot: full.next_pipe_slot,
            states: full
                .vms
                .iter()
                .map(|(id, state, _)| (*id, state.clone()))
                .collect(),
            pipes: full.pipes.into_iter().collect(),
            inherited_fd: full.inherited_fd.into_iter().collect(),
            instantiated: BTreeMap::default(),
            suspended: full
                .vms
                .into_iter()
                .map(|(id, _, snapshot)| (id, snapshot))
                .collect(),
            message_box: Arc::new(Mutex::new(Vec::new())),
            terminated_vms: full.terminated_vms.into_iter().collect(),
        }
    }

    /// Suspend current scheduler into a serializable full state
    pub fn suspend(mut self) -> Result<FullSuspendedState, Error> {
        let mut vms = Vec::with_capacity(self.states.len());
        let instantiated_ids: Vec<_> = self.instantiated.keys().cloned().collect();
        for id in instantiated_ids {
            self.suspend_vm(&id)?;
        }
        for (id, state) in self.states {
            let snapshot = self.suspended.remove(&id).unwrap();
            vms.push((id, state, snapshot));
        }
        Ok(FullSuspendedState {
            total_cycles: self.total_cycles,
            next_vm_id: self.next_vm_id,
            next_pipe_slot: self.next_pipe_slot,
            vms,
            pipes: self.pipes.into_iter().collect(),
            inherited_fd: self.inherited_fd.into_iter().collect(),
            terminated_vms: self.terminated_vms.into_iter().collect(),
        })
    }

    /// This is the only entrypoint for running the scheduler,
    /// both newly created instance and resumed instance are supported.
    /// It accepts 2 run mode, one can either limit the cycles to execute,
    /// or use a pause signal to trigger termination.
    ///
    /// Only when the execution terminates without VM errors, will this
    /// function return an exit code(could still be non-zero) and total
    /// consumed cycles.
    ///
    /// Err would be returned in the following cases:
    /// * Cycle limit reached, the returned error would be ckb_vm::Error::CyclesExceeded,
    /// * Pause trigger, the returned error would be ckb_vm::Error::Pause,
    /// * Other terminating errors
    pub fn run(&mut self, mode: RunMode) -> Result<(i8, Cycle), Error> {
        if self.states.is_empty() {
            // Booting phase, we will need to initialize the first VM.
            assert_eq!(
                self.boot_vm(&DataPieceId::Program, 0, u64::max_value(), &[])?,
                ROOT_VM_ID
            );
        }
        assert!(self.states.contains_key(&ROOT_VM_ID));

        let (pause, mut limit_cycles) = match mode {
            RunMode::LimitCycles(limit_cycles) => (Pause::new(), limit_cycles),
            RunMode::Pause(pause) => (pause, u64::max_value()),
        };

        while self.states[&ROOT_VM_ID] != VmState::Terminated {
            let consumed_cycles = self.iterate(pause.clone(), limit_cycles)?;
            limit_cycles = limit_cycles
                .checked_sub(consumed_cycles)
                .ok_or(Error::CyclesExceeded)?;
        }

        // At this point, root VM cannot be suspended
        let root_vm = &self.instantiated[&ROOT_VM_ID];
        Ok((root_vm.1.machine.exit_code(), self.total_cycles))
    }

    // This is internal function that does the actual VM execution loop.
    // Here both pause signal and limit_cycles are provided so as to simplify
    // branches.
    fn iterate(&mut self, pause: Pause, limit_cycles: Cycle) -> Result<Cycle, Error> {
        // 1. Process all pending VM reads & writes
        self.process_io()?;
        // 2. Run an actual VM
        // Find a runnable VM that has the largest ID
        let vm_id_to_run = self
            .states
            .iter()
            .rev()
            .filter(|(_, state)| matches!(state, VmState::Runnable))
            .map(|(id, _)| *id)
            .next();
        if vm_id_to_run.is_none() {
            return Err(Error::Unexpected(
                "A deadlock situation has been reached!".to_string(),
            ));
        }
        let vm_id_to_run = vm_id_to_run.unwrap();
        // log::debug!("Running VM {}", vm_id_to_run);
        let (result, consumed_cycles) = {
            self.ensure_vms_instantiated(&[vm_id_to_run])?;
            let (context, machine) = self.instantiated.get_mut(&vm_id_to_run).unwrap();
            context.set_base_cycles(self.total_cycles);
            machine.set_max_cycles(limit_cycles);
            machine.machine.set_pause(pause);
            let result = machine.run();
            let consumed_cycles = {
                let c = machine.machine.cycles();
                machine.machine.set_cycles(0);
                c
            };
            // This shall be the only place where total_cycles gets updated
            self.total_cycles = self
                .total_cycles
                .checked_add(consumed_cycles)
                .ok_or(Error::CyclesOverflow)?;
            (result, consumed_cycles)
        };
        // 3. Process message box, update VM states accordingly
        self.process_message_box()?;
        assert!(self.message_box.lock().expect("lock").is_empty());
        // log::debug!("VM states: {:?}", self.states);
        // log::debug!("Pipes and owners: {:?}", self.pipes);
        // 4. If the VM terminates, update VMs in join state, also closes its pipes
        match result {
            Ok(code) => {
                // log::debug!("VM {} terminates with code {}", vm_id_to_run, code);
                self.terminated_vms.insert(vm_id_to_run, code);
                // When root VM terminates, the execution stops immediately, we will purge
                // all non-root VMs, and only keep root VM in states.
                // When non-root VM terminates, we only purge the VM's own states.
                if vm_id_to_run == ROOT_VM_ID {
                    self.ensure_vms_instantiated(&[vm_id_to_run])?;
                    self.instantiated.retain(|id, _| *id == vm_id_to_run);
                    self.suspended.clear();
                    self.states.clear();
                    self.states.insert(vm_id_to_run, VmState::Terminated);
                } else {
                    let mut joining_vms: Vec<(VmId, u64)> = Vec::new();
                    self.states.iter().for_each(|(vm_id, state)| {
                        if let VmState::Wait {
                            target_vm_id,
                            exit_code_addr,
                        } = state
                        {
                            if *target_vm_id == vm_id_to_run {
                                joining_vms.push((*vm_id, *exit_code_addr));
                            }
                        }
                    });
                    // For all joining VMs, update exit code, then mark them as
                    // runnable state.
                    for (vm_id, exit_code_addr) in joining_vms {
                        self.ensure_vms_instantiated(&[vm_id])?;
                        let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                        machine
                            .machine
                            .memory_mut()
                            .store8(&exit_code_addr, &u64::from_i8(code))?;
                        machine.machine.set_register(A0, SUCCESS as u64);
                        self.states.insert(vm_id, VmState::Runnable);
                    }
                    // Close pipes
                    self.pipes.retain(|_, vm_id| *vm_id != vm_id_to_run);
                    // Clear terminated VM states
                    self.states.remove(&vm_id_to_run);
                    self.instantiated.remove(&vm_id_to_run);
                    self.suspended.remove(&vm_id_to_run);
                }
                Ok(consumed_cycles)
            }
            Err(Error::External(msg)) if msg == "YIELD" => Ok(consumed_cycles),
            Err(e) => Err(e),
        }
    }

    fn process_message_box(&mut self) -> Result<(), Error> {
        let messages: Vec<Message> = self.message_box.lock().expect("lock").drain(..).collect();
        for message in messages {
            match message {
                Message::Spawn(vm_id, args) => {
                    // All pipes must belong to the correct owner
                    for pipe in &args.pipes {
                        if !(self.pipes.contains_key(pipe) && (self.pipes[pipe] == vm_id)) {
                            return Err(Error::Unexpected(format!(
                                "VM {} does not own pipe {}!",
                                vm_id, pipe.0,
                            )));
                        }
                    }
                    // TODO: spawn limits
                    let spawned_vm_id =
                        self.boot_vm(&args.data_piece_id, args.offset, args.length, &args.argv)?;
                    // Move passed pipes from spawner to spawnee
                    for pipe in &args.pipes {
                        self.pipes.insert(*pipe, spawned_vm_id);
                    }
                    // here we keep the original version of file descriptors.
                    // if one fd is moved afterward, this inherited file descriptors doesn't change.
                    // log::info!(
                    //     "VmId = {} with Inherited file descriptor {:?}",
                    //     spawned_vm_id,
                    //     args.pipes
                    // );
                    self.inherited_fd.insert(spawned_vm_id, args.pipes.clone());

                    self.ensure_vms_instantiated(&[vm_id])?;
                    {
                        let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                        machine
                            .machine
                            .memory_mut()
                            .store64(&args.process_id_addr, &spawned_vm_id)?;
                        machine.machine.set_register(A0, SUCCESS as u64);
                    }
                }
                Message::Wait(vm_id, args) => {
                    if let Some(exit_code) = self.terminated_vms.get(&args.target_id).copied() {
                        self.ensure_vms_instantiated(&[vm_id])?;
                        {
                            let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                            machine
                                .machine
                                .memory_mut()
                                .store8(&args.exit_code_addr, &u64::from_i8(exit_code))?;
                            machine.machine.set_register(A0, SUCCESS as u64);
                            self.states.insert(vm_id, VmState::Runnable);
                        }
                        continue;
                    }
                    if !self.states.contains_key(&args.target_id) {
                        self.ensure_vms_instantiated(&[vm_id])?;
                        {
                            let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                            machine.machine.set_register(A0, WAIT_FAILURE as u64);
                        }
                        continue;
                    }
                    // Return code will be updated when the joining VM exits
                    self.states.insert(
                        vm_id,
                        VmState::Wait {
                            target_vm_id: args.target_id,
                            exit_code_addr: args.exit_code_addr,
                        },
                    );
                }
                Message::Pipe(vm_id, args) => {
                    // TODO: pipe limits
                    let (p1, p2, slot) = PipeId::create(self.next_pipe_slot);
                    self.next_pipe_slot = slot;
                    // log::debug!("VM {} creates pipes ({}, {})", vm_id, p1.0, p2.0);

                    self.pipes.insert(p1, vm_id);
                    self.pipes.insert(p2, vm_id);

                    self.ensure_vms_instantiated(&[vm_id])?;
                    {
                        let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                        machine
                            .machine
                            .memory_mut()
                            .store64(&args.pipe1_addr, &p1.0)?;
                        machine
                            .machine
                            .memory_mut()
                            .store64(&args.pipe2_addr, &p2.0)?;
                        machine.machine.set_register(A0, SUCCESS as u64);
                    }
                }
                Message::PipeRead(vm_id, args) => {
                    if !(self.pipes.contains_key(&args.pipe) && (self.pipes[&args.pipe] == vm_id)) {
                        self.ensure_vms_instantiated(&[vm_id])?;
                        {
                            let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                            machine.machine.set_register(A0, INVALID_PIPE as u64);
                        }
                        continue;
                    }
                    if !self.pipes.contains_key(&args.pipe.other_pipe()) {
                        self.ensure_vms_instantiated(&[vm_id])?;
                        {
                            let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                            machine.machine.set_register(A0, OTHER_END_CLOSED as u64);
                        }
                        continue;
                    }
                    // Return code will be updated when the read operation finishes
                    self.states.insert(
                        vm_id,
                        VmState::WaitForRead {
                            pipe: args.pipe,
                            length: args.length,
                            buffer_addr: args.buffer_addr,
                            length_addr: args.length_addr,
                        },
                    );
                }
                Message::PipeWrite(vm_id, args) => {
                    if !(self.pipes.contains_key(&args.pipe) && (self.pipes[&args.pipe] == vm_id)) {
                        self.ensure_vms_instantiated(&[vm_id])?;
                        {
                            let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                            machine.machine.set_register(A0, INVALID_PIPE as u64);
                        }
                        continue;
                    }
                    if !self.pipes.contains_key(&args.pipe.other_pipe()) {
                        self.ensure_vms_instantiated(&[vm_id])?;
                        {
                            let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                            machine.machine.set_register(A0, OTHER_END_CLOSED as u64);
                        }
                        continue;
                    }
                    // Return code will be updated when the write operation finishes
                    self.states.insert(
                        vm_id,
                        VmState::WaitForWrite {
                            pipe: args.pipe,
                            consumed: 0,
                            length: args.length,
                            buffer_addr: args.buffer_addr,
                            length_addr: args.length_addr,
                        },
                    );
                }
                Message::InheritedFileDescriptor(vm_id, args) => {
                    self.ensure_vms_instantiated(&[vm_id])?;
                    let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                    let PipeIoArgs {
                        buffer_addr,
                        length_addr,
                        ..
                    } = args;
                    let input_length = machine
                        .machine
                        .inner_mut()
                        .memory_mut()
                        .load64(&length_addr)?;
                    let inherited_fd = &self.inherited_fd[&vm_id];
                    let actual_length = inherited_fd.len() as u64;
                    if buffer_addr == 0 {
                        if input_length == 0 {
                            machine
                                .machine
                                .inner_mut()
                                .memory_mut()
                                .store64(&length_addr, &actual_length)?;
                            machine.machine.set_register(A0, SUCCESS as u64);
                        } else {
                            // TODO: in the previous convention
                            // https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0009-vm-syscalls/0009-vm-syscalls.md#partial-loading
                            // this will load data in to address 0 without notice. It is now marked as an error.
                            machine.machine.set_register(A0, INDEX_OUT_OF_BOUND as u64);
                        }
                        continue;
                    }
                    let mut buffer_addr2 = buffer_addr;
                    let copy_length = u64::min(input_length, actual_length);
                    for i in 0..copy_length {
                        let fd = inherited_fd[i as usize].0;
                        machine
                            .machine
                            .inner_mut()
                            .memory_mut()
                            .store64(&buffer_addr2, &fd)?;
                        buffer_addr2 += size_of::<u64>() as u64;
                    }
                    machine
                        .machine
                        .inner_mut()
                        .memory_mut()
                        .store64(&length_addr, &actual_length)?;
                    machine.machine.set_register(A0, SUCCESS as u64);
                }
                Message::Close(vm_id, fd) => {
                    self.ensure_vms_instantiated(&[vm_id])?;
                    let (_, machine) = self.instantiated.get_mut(&vm_id).unwrap();
                    if self.pipes.get(&fd) != Some(&vm_id) {
                        machine.machine.set_register(A0, INVALID_PIPE as u64);
                    } else {
                        self.pipes.remove(&fd);
                        machine.machine.set_register(A0, SUCCESS as u64);
                    }
                }
            }
        }
        Ok(())
    }

    fn process_io(&mut self) -> Result<(), Error> {
        let mut reads: HashMap<PipeId, (VmId, VmState)> = HashMap::default();
        let mut closed_pipes: Vec<VmId> = Vec::new();
        self.states.iter().for_each(|(vm_id, state)| {
            if let VmState::WaitForRead { pipe, .. } = state {
                if self.pipes.contains_key(&pipe.other_pipe()) {
                    reads.insert(*pipe, (*vm_id, state.clone()));
                } else {
                    closed_pipes.push(*vm_id);
                }
            }
        });
        let mut pairs: Vec<[(VmId, VmState); 2]> = Vec::new();
        self.states.iter().for_each(|(vm_id, state)| {
            if let VmState::WaitForWrite { pipe, .. } = state {
                if self.pipes.contains_key(&pipe.other_pipe()) {
                    if let Some((read_vm_id, read_state)) = reads.get(&pipe.other_pipe()) {
                        pairs.push([(*read_vm_id, read_state.clone()), (*vm_id, state.clone())]);
                    }
                } else {
                    closed_pipes.push(*vm_id);
                }
            }
        });
        // Finish read / write syscalls for pipes that are closed on the other end
        for vm_id in closed_pipes {
            match self.states[&vm_id].clone() {
                VmState::WaitForRead { length_addr, .. } => {
                    let (_, read_machine) = self.instantiated.get_mut(&vm_id).unwrap();
                    read_machine
                        .machine
                        .memory_mut()
                        .store64(&length_addr, &0)?;
                    read_machine.machine.set_register(A0, SUCCESS as u64);
                    self.states.insert(vm_id, VmState::Runnable);
                }
                VmState::WaitForWrite {
                    consumed,
                    length_addr,
                    ..
                } => {
                    let (_, write_machine) = self.instantiated.get_mut(&vm_id).unwrap();
                    write_machine
                        .machine
                        .memory_mut()
                        .store64(&length_addr, &consumed)?;
                    write_machine.machine.set_register(A0, SUCCESS as u64);
                    self.states.insert(vm_id, VmState::Runnable);
                }
                _ => (),
            }
        }
        // Transfering data from write pipes to read pipes
        for [(read_vm_id, read_state), (write_vm_id, write_state)] in pairs {
            let VmState::WaitForRead {
                length: read_length,
                buffer_addr: read_buffer_addr,
                length_addr: read_length_addr,
                ..
            } = read_state
            else {
                unreachable!()
            };
            let VmState::WaitForWrite {
                pipe: write_pipe,
                mut consumed,
                length: write_length,
                buffer_addr: write_buffer_addr,
                length_addr: write_length_addr,
            } = write_state
            else {
                unreachable!()
            };

            self.ensure_vms_instantiated(&[read_vm_id, write_vm_id])?;
            {
                let fillable = read_length;
                let consumable = write_length - consumed;
                let copiable = std::cmp::min(fillable, consumable);

                // Actual data copying
                // TODO: charge cycles
                let data = self
                    .instantiated
                    .get_mut(&write_vm_id)
                    .unwrap()
                    .1
                    .machine
                    .memory_mut()
                    .load_bytes(write_buffer_addr.wrapping_add(consumed), copiable)?;
                self.instantiated
                    .get_mut(&read_vm_id)
                    .unwrap()
                    .1
                    .machine
                    .memory_mut()
                    .store_bytes(read_buffer_addr, &data)?;

                // Read syscall terminates as soon as some data are filled
                let (_, read_machine) = self.instantiated.get_mut(&read_vm_id).unwrap();
                read_machine
                    .machine
                    .memory_mut()
                    .store64(&read_length_addr, &copiable)?;
                read_machine.machine.set_register(A0, SUCCESS as u64);
                self.states.insert(read_vm_id, VmState::Runnable);

                // Write syscall, however, terminates only when all the data
                // have been written, or when the pairing read pipe is closed.
                consumed += copiable;
                if consumed == write_length {
                    // write VM has fulfilled its write request
                    let (_, write_machine) = self.instantiated.get_mut(&write_vm_id).unwrap();
                    write_machine
                        .machine
                        .memory_mut()
                        .store64(&write_length_addr, &write_length)?;
                    write_machine.machine.set_register(A0, SUCCESS as u64);
                    self.states.insert(write_vm_id, VmState::Runnable);
                } else {
                    // Only update write VM state
                    self.states.insert(
                        write_vm_id,
                        VmState::WaitForWrite {
                            pipe: write_pipe,
                            consumed,
                            length: write_length,
                            buffer_addr: write_buffer_addr,
                            length_addr: write_length_addr,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    // Ensure VMs are instantiated
    fn ensure_vms_instantiated(&mut self, ids: &[VmId]) -> Result<(), Error> {
        if ids.len() > MAX_INSTANTIATED_VMS {
            return Err(Error::Unexpected(format!(
                "At most {} VMs can be instantiated but {} are requested!",
                MAX_INSTANTIATED_VMS,
                ids.len()
            )));
        }

        let mut uninstantiated_ids: Vec<VmId> = ids
            .iter()
            .filter(|id| !self.instantiated.contains_key(id))
            .copied()
            .collect();
        while (!uninstantiated_ids.is_empty()) && (self.instantiated.len() < MAX_INSTANTIATED_VMS) {
            let id = uninstantiated_ids.pop().unwrap();
            self.resume_vm(&id)?;
        }

        if !uninstantiated_ids.is_empty() {
            // instantiated is a BTreeMap, an iterator on it maintains key order to ensure deterministic behavior
            let suspendable_ids: Vec<VmId> = self
                .instantiated
                .keys()
                .filter(|id| !ids.contains(id))
                .copied()
                .collect();

            assert!(suspendable_ids.len() >= uninstantiated_ids.len());
            for i in 0..uninstantiated_ids.len() {
                self.suspend_vm(&suspendable_ids[i])?;
                self.resume_vm(&uninstantiated_ids[i])?;
            }
        }

        Ok(())
    }

    // Resume a suspended VM
    fn resume_vm(&mut self, id: &VmId) -> Result<(), Error> {
        println!("Resuming VM: {}", id);
        if !self.suspended.contains_key(id) {
            return Err(Error::Unexpected(format!("VM {:?} is not suspended!", id)));
        }
        let snapshot = &self.suspended[id];
        let (context, mut machine) = self.create_dummy_vm(id)?;
        {
            let mut sc = context.snapshot2_context().lock().expect("lock");
            sc.resume(&mut machine.machine, snapshot)?;
        }
        // TODO: charge cycles
        self.instantiated.insert(*id, (context, machine));
        self.suspended.remove(id);
        Ok(())
    }

    // Suspend an instantiated VM
    fn suspend_vm(&mut self, id: &VmId) -> Result<(), Error> {
        // log::debug!("Suspending VM: {}", id);
        if !self.instantiated.contains_key(id) {
            return Err(Error::Unexpected(format!(
                "VM {:?} is not instantiated!",
                id
            )));
        }
        // TODO: charge cycles
        let (context, machine) = self.instantiated.get_mut(id).unwrap();
        let snapshot = {
            let sc = context.snapshot2_context().lock().expect("lock");
            sc.make_snapshot(&mut machine.machine)?
        };
        self.suspended.insert(*id, snapshot);
        self.instantiated.remove(id);
        Ok(())
    }

    fn boot_vm(
        &mut self,
        data_piece_id: &DataPieceId,
        offset: u64,
        length: u64,
        args: &[Bytes],
    ) -> Result<VmId, Error> {
        // Newly booted VM will be instantiated by default
        while self.instantiated.len() >= MAX_INSTANTIATED_VMS {
            // instantiated is a BTreeMap, first_entry will maintain key order
            let id = *self.instantiated.first_entry().unwrap().key();
            self.suspend_vm(&id)?;
        }

        let id = self.next_vm_id;
        self.next_vm_id += 1;
        let (context, mut machine) = self.create_dummy_vm(&id)?;
        {
            let mut sc = context.snapshot2_context().lock().expect("lock");
            let (program, _) = sc.data_source().load_data(data_piece_id, offset, length)?;
            let metadata = parse_elf::<u64>(&program, machine.machine.version())?;
            let bytes = machine.load_program_with_metadata(&program, &metadata, args)?;
            sc.mark_program(&mut machine.machine, &metadata, data_piece_id, offset)?;
            machine
                .machine
                .add_cycles_no_checking(transferred_byte_cycles(bytes))?;
        }
        self.instantiated.insert(id, (context, machine));
        self.states.insert(id, VmState::Runnable);

        Ok(id)
    }

    // Create a new VM instance with syscalls attached
    fn create_dummy_vm(&self, id: &VmId) -> Result<(MachineContext<DL>, AsmMachine), Error> {
        // The code here looks slightly weird, since I don't want to copy over all syscall
        // impls here again. Ideally, this scheduler package should be merged with ckb-script,
        // or simply replace ckb-script. That way, the quirks here will be eliminated.
        let version = self.script_version;
        // log::debug!("Creating VM {} using version {:?}", id, version);
        let core_machine = AsmCoreMachine::new(
            version.vm_isa(),
            version.vm_version(),
            // We will update max_cycles for each machine when it gets a chance to run
            u64::max_value(),
        );

        let mut syscalls_generator = self.syscalls_generator.clone();
        syscalls_generator.vm_id = *id;
        let mut machine_context =
            MachineContext::new(*id, self.message_box.clone(), self.tx_data.clone(), version);
        machine_context.base_cycles = Arc::clone(&self.syscalls_generator.base_cycles);

        let machine_builder = DefaultMachineBuilder::new(core_machine)
            .instruction_cycle_func(Box::new(estimate_cycles))
            // ckb-vm iterates syscalls in insertion order, by putting
            // MachineContext at the first place, we can override other
            // syscalls with implementations from MachineContext. For example,
            // we can override load_cell_data syscall with a new implementation.
            .syscall(Box::new(machine_context.clone()));
        let machine_builder = syscalls_generator
            .generate_root_syscalls(version, &self.tx_data.script_group, Default::default())
            .into_iter()
            .fold(machine_builder, |builder, syscall| builder.syscall(syscall));
        let default_machine = machine_builder.build();
        Ok((machine_context, AsmMachine::new(default_machine)))
    }
}
