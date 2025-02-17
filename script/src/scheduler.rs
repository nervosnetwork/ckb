use crate::cost_model::transferred_byte_cycles;
use crate::syscalls::{
    EXEC_LOAD_ELF_V2_CYCLES_BASE, INVALID_FD, MAX_FDS_CREATED, MAX_VMS_SPAWNED, OTHER_END_CLOSED,
    SPAWN_EXTRA_CYCLES_BASE, SUCCESS, WAIT_FAILURE,
};
use crate::types::MachineContext;
use crate::verify::TransactionScriptsSyscallsGenerator;
use crate::ScriptVersion;

use crate::types::{
    CoreMachineType, DataLocation, DataPieceId, Fd, FdArgs, FullSuspendedState, Machine, Message,
    ReadState, RunMode, TxData, VmId, VmState, WriteState, FIRST_FD_SLOT, FIRST_VM_ID,
};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::core::Cycle;
use ckb_vm::snapshot2::Snapshot2Context;
use ckb_vm::{
    bytes::Bytes,
    cost_model::estimate_cycles,
    elf::parse_elf,
    machine::{CoreMachine, DefaultMachineBuilder, Pause, SupportMachine},
    memory::Memory,
    registers::A0,
    snapshot2::Snapshot2,
    Error, FlattenedArgsReader, Register,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// Root process's id.
pub const ROOT_VM_ID: VmId = FIRST_VM_ID;
/// The maximum number of VMs that can be created at the same time.
pub const MAX_VMS_COUNT: u64 = 16;
/// The maximum number of instantiated VMs.
pub const MAX_INSTANTIATED_VMS: usize = 4;
/// The maximum number of fds.
pub const MAX_FDS: u64 = 64;

/// A single Scheduler instance is used to verify a single script
/// within a CKB transaction.
///
/// A scheduler holds & manipulates a core, the scheduler also holds
/// all CKB-VM machines, each CKB-VM machine also gets a mutable reference
/// of the core for IO operations.
pub struct Scheduler<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    /// Context data for current running transaction & script.
    pub tx_data: TxData<DL>,
    /// In fact, Scheduler here has the potential to totally replace
    /// TransactionScriptsVerifier, nonetheless much of current syscall
    /// implementation is strictly tied to TransactionScriptsVerifier, we
    /// are using it here to save some extra code.
    pub script_version: ScriptVersion,
    /// Generate system calls.
    pub syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,

    /// Total cycles.
    pub total_cycles: Cycle,
    /// Current iteration cycles. This value is periodically added to
    /// total_cycles and cleared
    pub current_iteration_cycles: Cycle,
    /// Next vm id used by spawn.
    pub next_vm_id: VmId,
    /// Next fd used by pipe.
    pub next_fd_slot: u64,
    /// Used to store VM state.
    pub states: BTreeMap<VmId, VmState>,
    /// Used to confirm the owner of fd.
    pub fds: BTreeMap<Fd, VmId>,
    /// Verify the VM's inherited fd list.
    pub inherited_fd: BTreeMap<VmId, Vec<Fd>>,
    /// Instantiated vms.
    pub instantiated: BTreeMap<VmId, (MachineContext<DL>, Machine)>,
    /// Suspended vms.
    pub suspended: BTreeMap<VmId, Snapshot2<DataPieceId>>,
    /// Terminated vms.
    pub terminated_vms: BTreeMap<VmId, i8>,

    /// MessageBox is expected to be empty before returning from `run`
    /// function, there is no need to persist messages.
    pub message_box: Arc<Mutex<Vec<Message>>>,
}

impl<DL> Scheduler<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    /// Create a new scheduler from empty state
    pub fn new(
        tx_data: TxData<DL>,
        script_version: ScriptVersion,
        syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,
    ) -> Self {
        let message_box = Arc::clone(&syscalls_generator.message_box);
        Self {
            tx_data,
            script_version,
            syscalls_generator,
            total_cycles: 0,
            current_iteration_cycles: 0,
            next_vm_id: FIRST_VM_ID,
            next_fd_slot: FIRST_FD_SLOT,
            states: BTreeMap::default(),
            fds: BTreeMap::default(),
            inherited_fd: BTreeMap::default(),
            instantiated: BTreeMap::default(),
            suspended: BTreeMap::default(),
            message_box,
            terminated_vms: BTreeMap::default(),
        }
    }

    /// Return total cycles.
    pub fn consumed_cycles(&self) -> Cycle {
        self.total_cycles
    }

    /// Add cycles to total cycles.
    pub fn consumed_cycles_add(&mut self, cycles: Cycle) -> Result<(), Error> {
        self.total_cycles = self
            .total_cycles
            .checked_add(cycles)
            .ok_or(Error::CyclesExceeded)?;
        Ok(())
    }

    /// Resume a previously suspended scheduler state
    pub fn resume(
        tx_data: TxData<DL>,
        script_version: ScriptVersion,
        syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,
        full: FullSuspendedState,
    ) -> Self {
        let message_box = Arc::clone(&syscalls_generator.message_box);
        let mut scheduler = Self {
            tx_data,
            script_version,
            syscalls_generator,
            total_cycles: full.total_cycles,
            current_iteration_cycles: 0,
            next_vm_id: full.next_vm_id,
            next_fd_slot: full.next_fd_slot,
            states: full
                .vms
                .iter()
                .map(|(id, state, _)| (*id, state.clone()))
                .collect(),
            fds: full.fds.into_iter().collect(),
            inherited_fd: full.inherited_fd.into_iter().collect(),
            instantiated: BTreeMap::default(),
            suspended: full
                .vms
                .into_iter()
                .map(|(id, _, snapshot)| (id, snapshot))
                .collect(),
            message_box,
            terminated_vms: full.terminated_vms.into_iter().collect(),
        };
        scheduler
            .ensure_vms_instantiated(&full.instantiated_ids)
            .unwrap();
        scheduler
    }

    /// Suspend current scheduler into a serializable full state
    pub fn suspend(mut self) -> Result<FullSuspendedState, Error> {
        assert!(self.message_box.lock().expect("lock").is_empty());
        let mut vms = Vec::with_capacity(self.states.len());
        let instantiated_ids: Vec<_> = self.instantiated.keys().cloned().collect();
        for id in &instantiated_ids {
            self.suspend_vm(id)?;
        }
        for (id, state) in self.states {
            let snapshot = self
                .suspended
                .remove(&id)
                .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
            vms.push((id, state, snapshot));
        }
        Ok(FullSuspendedState {
            total_cycles: self.total_cycles,
            next_vm_id: self.next_vm_id,
            next_fd_slot: self.next_fd_slot,
            vms,
            fds: self.fds.into_iter().collect(),
            inherited_fd: self.inherited_fd.into_iter().collect(),
            terminated_vms: self.terminated_vms.into_iter().collect(),
            instantiated_ids,
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
                self.boot_vm(
                    &DataLocation {
                        data_piece_id: DataPieceId::Program,
                        offset: 0,
                        length: u64::MAX,
                    },
                    None
                )?,
                ROOT_VM_ID
            );
        }
        assert!(self.states.contains_key(&ROOT_VM_ID));

        let (pause, mut limit_cycles) = match mode {
            RunMode::LimitCycles(limit_cycles) => (Pause::new(), limit_cycles),
            RunMode::Pause(pause) => (pause, u64::MAX),
        };

        while self.states[&ROOT_VM_ID] != VmState::Terminated {
            self.current_iteration_cycles = 0;
            let iterate_return = self.iterate(pause.clone(), limit_cycles);
            self.consumed_cycles_add(self.current_iteration_cycles)?;
            limit_cycles = limit_cycles
                .checked_sub(self.current_iteration_cycles)
                .ok_or(Error::CyclesExceeded)?;
            iterate_return?;
        }

        // At this point, root VM cannot be suspended
        let root_vm = &self.instantiated[&ROOT_VM_ID];
        Ok((root_vm.1.machine.exit_code(), self.total_cycles))
    }

    /// Returns the machine that needs to be executed in the current iterate.
    pub fn iterate_prepare_machine(
        &mut self,
        pause: Pause,
        limit_cycles: Cycle,
    ) -> Result<(u64, &mut Machine), Error> {
        // Process all pending VM reads & writes.
        self.process_io()?;
        // Find a runnable VM that has the largest ID.
        let vm_id_to_run = self
            .states
            .iter()
            .rev()
            .filter(|(_, state)| matches!(state, VmState::Runnable))
            .map(|(id, _)| *id)
            .next();
        let vm_id_to_run = vm_id_to_run.ok_or_else(|| {
            Error::Unexpected("A deadlock situation has been reached!".to_string())
        })?;
        let total_cycles = self.total_cycles;
        let (context, machine) = self.ensure_get_instantiated(&vm_id_to_run)?;
        context.set_base_cycles(total_cycles);
        machine.set_max_cycles(limit_cycles);
        machine.machine.set_pause(pause);
        Ok((vm_id_to_run, machine))
    }

    /// Process machine execution results in the current iterate.
    pub fn iterate_process_results(
        &mut self,
        vm_id_to_run: u64,
        result: Result<i8, Error>,
        cycles: u64,
    ) -> Result<(), Error> {
        self.current_iteration_cycles = self
            .current_iteration_cycles
            .checked_add(cycles)
            .ok_or(Error::CyclesOverflow)?;
        // Process message box, update VM states accordingly
        self.process_message_box()?;
        assert!(self.message_box.lock().expect("lock").is_empty());
        // If the VM terminates, update VMs in join state, also closes its fds
        match result {
            Ok(code) => {
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
                    let joining_vms: Vec<(VmId, u64)> = self
                        .states
                        .iter()
                        .filter_map(|(vm_id, state)| match state {
                            VmState::Wait {
                                target_vm_id,
                                exit_code_addr,
                            } if *target_vm_id == vm_id_to_run => Some((*vm_id, *exit_code_addr)),
                            _ => None,
                        })
                        .collect();
                    // For all joining VMs, update exit code, then mark them as
                    // runnable state.
                    for (vm_id, exit_code_addr) in joining_vms {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .machine
                            .memory_mut()
                            .store8(&exit_code_addr, &u64::from_i8(code))?;
                        machine.machine.set_register(A0, SUCCESS as u64);
                        self.states.insert(vm_id, VmState::Runnable);
                    }
                    // Close fds
                    self.fds.retain(|_, vm_id| *vm_id != vm_id_to_run);
                    // Clear terminated VM states
                    self.states.remove(&vm_id_to_run);
                    self.instantiated.remove(&vm_id_to_run);
                    self.suspended.remove(&vm_id_to_run);
                }
                Ok(())
            }
            Err(Error::Yield) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // This is internal function that does the actual VM execution loop.
    // Here both pause signal and limit_cycles are provided so as to simplify
    // branches.
    fn iterate(&mut self, pause: Pause, limit_cycles: Cycle) -> Result<(), Error> {
        let (id, vm) = self.iterate_prepare_machine(pause, limit_cycles)?;
        let result = vm.run();
        let cycles = vm.machine.cycles();
        vm.machine.set_cycles(0);
        self.iterate_process_results(id, result, cycles)
    }

    fn process_message_box(&mut self) -> Result<(), Error> {
        let messages: Vec<Message> = self.message_box.lock().expect("lock").drain(..).collect();
        for message in messages {
            match message {
                Message::ExecV2(vm_id, args) => {
                    let (old_context, old_machine) = self
                        .instantiated
                        .get_mut(&vm_id)
                        .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
                    old_machine
                        .machine
                        .add_cycles_no_checking(EXEC_LOAD_ELF_V2_CYCLES_BASE)?;
                    let old_cycles = old_machine.machine.cycles();
                    let max_cycles = old_machine.machine.max_cycles();
                    let program = {
                        let mut sc = old_context.snapshot2_context().lock().expect("lock");
                        sc.load_data(
                            &args.location.data_piece_id,
                            args.location.offset,
                            args.location.length,
                        )?
                        .0
                    };
                    let (context, mut new_machine) = self.create_dummy_vm(&vm_id)?;
                    new_machine.set_max_cycles(max_cycles);
                    new_machine.machine.add_cycles_no_checking(old_cycles)?;
                    self.load_vm_program(
                        &context,
                        &mut new_machine,
                        &args.location,
                        program,
                        Some((vm_id, args.argc, args.argv)),
                    )?;
                    // The insert operation removes the old vm instance and adds the new vm instance.
                    debug_assert!(self.instantiated.contains_key(&vm_id));
                    self.instantiated.insert(vm_id, (context, new_machine));
                }
                Message::Spawn(vm_id, args) => {
                    // All fds must belong to the correct owner
                    if args.fds.iter().any(|fd| self.fds.get(fd) != Some(&vm_id)) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, INVALID_FD as u64);
                        continue;
                    }
                    if self.suspended.len() + self.instantiated.len() > MAX_VMS_COUNT as usize {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, MAX_VMS_SPAWNED as u64);
                        continue;
                    }
                    let spawned_vm_id =
                        self.boot_vm(&args.location, Some((vm_id, args.argc, args.argv)))?;
                    // Move passed fds from spawner to spawnee
                    for fd in &args.fds {
                        self.fds.insert(*fd, spawned_vm_id);
                    }
                    // Here we keep the original version of file descriptors.
                    // If one fd is moved afterward, this inherited file descriptors doesn't change.
                    self.inherited_fd.insert(spawned_vm_id, args.fds.clone());

                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    machine
                        .machine
                        .memory_mut()
                        .store64(&args.process_id_addr, &spawned_vm_id)?;
                    machine.machine.set_register(A0, SUCCESS as u64);
                }
                Message::Wait(vm_id, args) => {
                    if let Some(exit_code) = self.terminated_vms.get(&args.target_id).copied() {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .machine
                            .memory_mut()
                            .store8(&args.exit_code_addr, &u64::from_i8(exit_code))?;
                        machine.machine.set_register(A0, SUCCESS as u64);
                        self.states.insert(vm_id, VmState::Runnable);
                        self.terminated_vms.retain(|id, _| id != &args.target_id);
                        continue;
                    }
                    if !self.states.contains_key(&args.target_id) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, WAIT_FAILURE as u64);
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
                    if self.fds.len() as u64 >= MAX_FDS {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, MAX_FDS_CREATED as u64);
                        continue;
                    }
                    let (p1, p2, slot) = Fd::create(self.next_fd_slot);
                    self.next_fd_slot = slot;
                    self.fds.insert(p1, vm_id);
                    self.fds.insert(p2, vm_id);
                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    machine
                        .machine
                        .memory_mut()
                        .store64(&args.fd1_addr, &p1.0)?;
                    machine
                        .machine
                        .memory_mut()
                        .store64(&args.fd2_addr, &p2.0)?;
                    machine.machine.set_register(A0, SUCCESS as u64);
                }
                Message::FdRead(vm_id, args) => {
                    if !(self.fds.contains_key(&args.fd) && (self.fds[&args.fd] == vm_id)) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, INVALID_FD as u64);
                        continue;
                    }
                    if !self.fds.contains_key(&args.fd.other_fd()) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, OTHER_END_CLOSED as u64);
                        continue;
                    }
                    // Return code will be updated when the read operation finishes
                    self.states.insert(
                        vm_id,
                        VmState::WaitForRead(ReadState {
                            fd: args.fd,
                            length: args.length,
                            buffer_addr: args.buffer_addr,
                            length_addr: args.length_addr,
                        }),
                    );
                }
                Message::FdWrite(vm_id, args) => {
                    if !(self.fds.contains_key(&args.fd) && (self.fds[&args.fd] == vm_id)) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, INVALID_FD as u64);
                        continue;
                    }
                    if !self.fds.contains_key(&args.fd.other_fd()) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, OTHER_END_CLOSED as u64);
                        continue;
                    }
                    // Return code will be updated when the write operation finishes
                    self.states.insert(
                        vm_id,
                        VmState::WaitForWrite(WriteState {
                            fd: args.fd,
                            consumed: 0,
                            length: args.length,
                            buffer_addr: args.buffer_addr,
                            length_addr: args.length_addr,
                        }),
                    );
                }
                Message::InheritedFileDescriptor(vm_id, args) => {
                    let inherited_fd = if vm_id == ROOT_VM_ID {
                        Vec::new()
                    } else {
                        self.inherited_fd[&vm_id].clone()
                    };
                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    let FdArgs {
                        buffer_addr,
                        length_addr,
                        ..
                    } = args;
                    let full_length = machine
                        .machine
                        .inner_mut()
                        .memory_mut()
                        .load64(&length_addr)?;
                    let real_length = inherited_fd.len() as u64;
                    let copy_length = u64::min(full_length, real_length);
                    for i in 0..copy_length {
                        let fd = inherited_fd[i as usize].0;
                        let addr = buffer_addr.checked_add(i * 8).ok_or(Error::MemOutOfBound)?;
                        machine
                            .machine
                            .inner_mut()
                            .memory_mut()
                            .store64(&addr, &fd)?;
                    }
                    machine
                        .machine
                        .inner_mut()
                        .memory_mut()
                        .store64(&length_addr, &real_length)?;
                    machine.machine.set_register(A0, SUCCESS as u64);
                }
                Message::Close(vm_id, fd) => {
                    if self.fds.get(&fd) != Some(&vm_id) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, INVALID_FD as u64);
                    } else {
                        self.fds.remove(&fd);
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.machine.set_register(A0, SUCCESS as u64);
                    }
                }
            }
        }
        Ok(())
    }

    fn process_io(&mut self) -> Result<(), Error> {
        let mut reads: HashMap<Fd, (VmId, ReadState)> = HashMap::default();
        let mut closed_fds: Vec<VmId> = Vec::new();
        self.states.iter().for_each(|(vm_id, state)| {
            if let VmState::WaitForRead(inner_state) = state {
                if self.fds.contains_key(&inner_state.fd.other_fd()) {
                    reads.insert(inner_state.fd, (*vm_id, inner_state.clone()));
                } else {
                    closed_fds.push(*vm_id);
                }
            }
        });
        let mut pairs: Vec<(VmId, ReadState, VmId, WriteState)> = Vec::new();
        self.states.iter().for_each(|(vm_id, state)| {
            if let VmState::WaitForWrite(inner_state) = state {
                if self.fds.contains_key(&inner_state.fd.other_fd()) {
                    if let Some((read_vm_id, read_state)) = reads.get(&inner_state.fd.other_fd()) {
                        pairs.push((*read_vm_id, read_state.clone(), *vm_id, inner_state.clone()));
                    }
                } else {
                    closed_fds.push(*vm_id);
                }
            }
        });
        // Finish read / write syscalls for fds that are closed on the other end
        for vm_id in closed_fds {
            match self.states[&vm_id].clone() {
                VmState::WaitForRead(ReadState { length_addr, .. }) => {
                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    machine.machine.memory_mut().store64(&length_addr, &0)?;
                    machine.machine.set_register(A0, SUCCESS as u64);
                    self.states.insert(vm_id, VmState::Runnable);
                }
                VmState::WaitForWrite(WriteState {
                    consumed,
                    length_addr,
                    ..
                }) => {
                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    machine
                        .machine
                        .memory_mut()
                        .store64(&length_addr, &consumed)?;
                    machine.machine.set_register(A0, SUCCESS as u64);
                    self.states.insert(vm_id, VmState::Runnable);
                }
                _ => (),
            }
        }
        // Transferring data from write fds to read fds
        for (read_vm_id, read_state, write_vm_id, write_state) in pairs {
            let ReadState {
                length: read_length,
                buffer_addr: read_buffer_addr,
                length_addr: read_length_addr,
                ..
            } = read_state;
            let WriteState {
                fd: write_fd,
                mut consumed,
                length: write_length,
                buffer_addr: write_buffer_addr,
                length_addr: write_length_addr,
            } = write_state;

            self.ensure_vms_instantiated(&[read_vm_id, write_vm_id])?;
            {
                let fillable = read_length;
                let consumable = write_length - consumed;
                let copiable = std::cmp::min(fillable, consumable);

                // Actual data copying
                let (_, write_machine) = self
                    .instantiated
                    .get_mut(&write_vm_id)
                    .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
                write_machine
                    .machine
                    .add_cycles_no_checking(transferred_byte_cycles(copiable))?;
                let data = write_machine
                    .machine
                    .memory_mut()
                    .load_bytes(write_buffer_addr.wrapping_add(consumed), copiable)?;
                let (_, read_machine) = self
                    .instantiated
                    .get_mut(&read_vm_id)
                    .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
                read_machine
                    .machine
                    .add_cycles_no_checking(transferred_byte_cycles(copiable))?;
                read_machine
                    .machine
                    .memory_mut()
                    .store_bytes(read_buffer_addr, &data)?;
                // Read syscall terminates as soon as some data are filled
                read_machine
                    .machine
                    .memory_mut()
                    .store64(&read_length_addr, &copiable)?;
                read_machine.machine.set_register(A0, SUCCESS as u64);
                self.states.insert(read_vm_id, VmState::Runnable);

                // Write syscall, however, terminates only when all the data
                // have been written, or when the pairing read fd is closed.
                consumed += copiable;
                if consumed == write_length {
                    // Write VM has fulfilled its write request
                    let (_, write_machine) = self
                        .instantiated
                        .get_mut(&write_vm_id)
                        .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
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
                        VmState::WaitForWrite(WriteState {
                            fd: write_fd,
                            consumed,
                            length: write_length,
                            buffer_addr: write_buffer_addr,
                            length_addr: write_length_addr,
                        }),
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
            let id = uninstantiated_ids
                .pop()
                .ok_or_else(|| Error::Unexpected("Map should not be empty".to_string()))?;
            self.resume_vm(&id)?;
        }

        if !uninstantiated_ids.is_empty() {
            // Instantiated is a BTreeMap, an iterator on it maintains key order to ensure deterministic behavior
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

    // Ensure corresponding VM is instantiated and return a mutable reference to it
    fn ensure_get_instantiated(
        &mut self,
        id: &VmId,
    ) -> Result<&mut (MachineContext<DL>, Machine), Error> {
        self.ensure_vms_instantiated(&[*id])?;
        self.instantiated
            .get_mut(id)
            .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))
    }

    // Resume a suspended VM
    fn resume_vm(&mut self, id: &VmId) -> Result<(), Error> {
        if !self.suspended.contains_key(id) {
            return Err(Error::Unexpected(format!("VM {:?} is not suspended!", id)));
        }
        let snapshot = &self.suspended[id];
        self.current_iteration_cycles = self
            .current_iteration_cycles
            .checked_add(SPAWN_EXTRA_CYCLES_BASE)
            .ok_or(Error::CyclesExceeded)?;
        let (context, mut machine) = self.create_dummy_vm(id)?;
        {
            let mut sc = context.snapshot2_context().lock().expect("lock");
            sc.resume(&mut machine.machine, snapshot)?;
        }
        self.instantiated.insert(*id, (context, machine));
        self.suspended.remove(id);
        Ok(())
    }

    // Suspend an instantiated VM
    fn suspend_vm(&mut self, id: &VmId) -> Result<(), Error> {
        if !self.instantiated.contains_key(id) {
            return Err(Error::Unexpected(format!(
                "VM {:?} is not instantiated!",
                id
            )));
        }
        self.current_iteration_cycles = self
            .current_iteration_cycles
            .checked_add(SPAWN_EXTRA_CYCLES_BASE)
            .ok_or(Error::CyclesExceeded)?;
        let (context, machine) = self
            .instantiated
            .get_mut(id)
            .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
        let snapshot = {
            let sc = context.snapshot2_context().lock().expect("lock");
            sc.make_snapshot(&mut machine.machine)?
        };
        self.suspended.insert(*id, snapshot);
        self.instantiated.remove(id);
        Ok(())
    }

    /// Boot a vm by given program and args.
    pub fn boot_vm(
        &mut self,
        location: &DataLocation,
        args: Option<(u64, u64, u64)>,
    ) -> Result<VmId, Error> {
        let id = self.next_vm_id;
        self.next_vm_id += 1;
        let (context, mut machine) = self.create_dummy_vm(&id)?;
        let (program, _) = {
            let mut sc = context.snapshot2_context().lock().expect("lock");
            sc.load_data(&location.data_piece_id, location.offset, location.length)?
        };
        self.load_vm_program(&context, &mut machine, location, program, args)?;
        // Newly booted VM will be instantiated by default
        while self.instantiated.len() >= MAX_INSTANTIATED_VMS {
            // Instantiated is a BTreeMap, first_entry will maintain key order
            let id = *self
                .instantiated
                .first_entry()
                .ok_or_else(|| Error::Unexpected("Map should not be empty".to_string()))?
                .key();
            self.suspend_vm(&id)?;
        }
        self.instantiated.insert(id, (context, machine));
        self.states.insert(id, VmState::Runnable);

        Ok(id)
    }

    // Load the program into an empty vm.
    fn load_vm_program(
        &mut self,
        context: &MachineContext<DL>,
        machine: &mut Machine,
        location: &DataLocation,
        program: Bytes,
        args: Option<(u64, u64, u64)>,
    ) -> Result<u64, Error> {
        let metadata = parse_elf::<u64>(&program, machine.machine.version())?;
        let bytes = match args {
            Some((vm_id, argc, argv)) => {
                let (_, machine_from) = self.ensure_get_instantiated(&vm_id)?;
                let argv = FlattenedArgsReader::new(machine_from.machine.memory_mut(), argc, argv);
                machine.load_program_with_metadata(&program, &metadata, argv)?
            }
            None => machine.load_program_with_metadata(&program, &metadata, vec![].into_iter())?,
        };
        let mut sc = context.snapshot2_context().lock().expect("lock");
        sc.mark_program(
            &mut machine.machine,
            &metadata,
            &location.data_piece_id,
            location.offset,
        )?;
        machine
            .machine
            .add_cycles_no_checking(transferred_byte_cycles(bytes))?;
        Ok(bytes)
    }

    // Create a new VM instance with syscalls attached
    fn create_dummy_vm(&self, id: &VmId) -> Result<(MachineContext<DL>, Machine), Error> {
        // The code here looks slightly weird, since I don't want to copy over all syscall
        // impls here again. Ideally, this scheduler package should be merged with ckb-script,
        // or simply replace ckb-script. That way, the quirks here will be eliminated.
        let version = self.script_version;
        let core_machine = CoreMachineType::new(
            version.vm_isa(),
            version.vm_version(),
            // We will update max_cycles for each machine when it gets a chance to run
            u64::MAX,
        );
        let snapshot2_context = Arc::new(Mutex::new(Snapshot2Context::new(self.tx_data.clone())));
        let mut syscalls_generator = self.syscalls_generator.clone();
        syscalls_generator.vm_id = *id;
        let mut machine_context = MachineContext::new(self.tx_data.clone());
        machine_context.base_cycles = Arc::clone(&self.syscalls_generator.base_cycles);
        machine_context.snapshot2_context = Arc::clone(&snapshot2_context);

        let machine_builder = DefaultMachineBuilder::new(core_machine)
            .instruction_cycle_func(Box::new(estimate_cycles));
        let machine_builder = syscalls_generator
            .generate_syscalls(
                version,
                &self.tx_data.script_group,
                Arc::clone(&snapshot2_context),
            )
            .into_iter()
            .fold(machine_builder, |builder, syscall| builder.syscall(syscall));
        let default_machine = machine_builder.build();
        Ok((machine_context, Machine::new(default_machine)))
    }
}
