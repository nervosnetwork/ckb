use crate::cost_model::transferred_byte_cycles;
use crate::syscalls::{
    EXEC_LOAD_ELF_V2_CYCLES_BASE, INVALID_FD, MAX_FDS_CREATED, MAX_VMS_SPAWNED, OTHER_END_CLOSED,
    SPAWN_EXTRA_CYCLES_BASE, SUCCESS, WAIT_FAILURE,
};

use crate::types::{
    DataLocation, DataPieceId, FIRST_FD_SLOT, FIRST_VM_ID, Fd, FdArgs, FullSuspendedState,
    IterationResult, Message, ReadState, RunMode, SgData, SyscallGenerator, TerminatedResult,
    VmArgs, VmContext, VmId, VmState, WriteState,
};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::core::Cycle;
use ckb_vm::snapshot2::Snapshot2Context;
use ckb_vm::{
    Error, FlattenedArgsReader, Register,
    bytes::Bytes,
    cost_model::estimate_cycles,
    elf::parse_elf,
    machine::{CoreMachine, DefaultMachineBuilder, DefaultMachineRunner, Pause, SupportMachine},
    memory::Memory,
    registers::A0,
    snapshot2::Snapshot2,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

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
pub struct Scheduler<DL, V, M>
where
    DL: CellDataProvider,
    M: DefaultMachineRunner,
{
    /// Immutable context data for current running transaction & script.
    sg_data: SgData<DL>,

    /// Syscall generator
    syscall_generator: SyscallGenerator<DL, V, M::Inner>,
    /// Syscall generator context
    syscall_context: V,

    /// Total cycles. When a scheduler executes, there are 3 variables
    /// that might all contain charged cycles: +total_cycles+,
    /// +iteration_cycles+ and +machine.cycles()+ from the current
    /// executing virtual machine. At any given time, the sum of all 3
    /// variables here, represent the total consumed cycles by the current
    /// scheduler.
    /// But there are also exceptions: at certain period of time, the cycles
    /// stored in `machine.cycles()` are moved over to +iteration_cycles+,
    /// the cycles stored in +iteration_cycles+ would also be moved over to
    /// +total_cycles+:
    ///
    /// * The current running virtual machine would contain consumed
    ///   cycles in its own machine.cycles() structure.
    /// * +iteration_cycles+ holds the current consumed cycles each time
    ///   we executed a virtual machine(also named an iteration). It will
    ///   always be zero before each iteration(i.e., before each VM starts
    ///   execution). When a virtual machine finishes execution, the cycles
    ///   stored in `machine.cycles()` will be moved over to +iteration_cycles+.
    ///   `machine.cycles()` will then be reset to zero.
    /// * Processing messages in the message box would alao charge cycles
    ///   for operations, such as suspending/resuming VMs, transferring data
    ///   etc. Those cycles were added to +iteration_cycles+ directly. When all
    ///   postprocessing work is completed, the cycles consumed in
    ///   +iteration_cycles+ will then be moved to +total_cycles+.
    ///   +iteration_cycles+ will then be reset to zero.
    ///
    /// One can consider that +total_cycles+ contains the total cycles
    /// consumed in current scheduler, when the scheduler is not busy executing.
    total_cycles: Arc<AtomicU64>,
    /// Iteration cycles, see +total_cycles+ on its usage
    iteration_cycles: Cycle,
    /// Next vm id used by spawn.
    next_vm_id: VmId,
    /// Next fd used by pipe.
    next_fd_slot: u64,
    /// Used to store VM state.
    states: BTreeMap<VmId, VmState>,
    /// Used to confirm the owner of fd.
    fds: BTreeMap<Fd, VmId>,
    /// Verify the VM's inherited fd list.
    inherited_fd: BTreeMap<VmId, Vec<Fd>>,
    /// Instantiated vms.
    instantiated: BTreeMap<VmId, (VmContext<DL>, M)>,
    /// Suspended vms.
    suspended: BTreeMap<VmId, Snapshot2<DataPieceId>>,
    /// Terminated vms.
    terminated_vms: BTreeMap<VmId, i8>,

    /// MessageBox is expected to be empty before returning from `run`
    /// function, there is no need to persist messages.
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl<DL, V, M> Scheduler<DL, V, M>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Clone,
    V: Clone,
    M: DefaultMachineRunner,
{
    /// Create a new scheduler from empty state
    pub fn new(
        sg_data: SgData<DL>,
        syscall_generator: SyscallGenerator<DL, V, M::Inner>,
        syscall_context: V,
    ) -> Self {
        Self {
            sg_data,
            syscall_generator,
            syscall_context,
            total_cycles: Arc::new(AtomicU64::new(0)),
            iteration_cycles: 0,
            next_vm_id: FIRST_VM_ID,
            next_fd_slot: FIRST_FD_SLOT,
            states: BTreeMap::default(),
            fds: BTreeMap::default(),
            inherited_fd: BTreeMap::default(),
            instantiated: BTreeMap::default(),
            suspended: BTreeMap::default(),
            message_box: Arc::new(Mutex::new(Vec::new())),
            terminated_vms: BTreeMap::default(),
        }
    }

    /// Return total cycles.
    pub fn consumed_cycles(&self) -> Cycle {
        self.total_cycles.load(Ordering::Acquire)
    }

    /// Fetch specified VM state
    pub fn state(&self, vm_id: &VmId) -> Option<VmState> {
        self.states.get(vm_id).cloned()
    }

    /// Access the SgData data structure
    pub fn sg_data(&self) -> &SgData<DL> {
        &self.sg_data
    }

    /// This function provides a peek into one of the current created
    /// VM. Depending on the actual state, the VM might either be instantiated
    /// or suspended. As a result, 2 callback functions must be provided to handle
    /// both cases. The function only provides a *peek*, meaning the caller must
    /// not make any changes to an instantiated VMs. the VM is passed as a mutable
    /// reference only because memory load functions in CKB-VM require mutable
    /// references. It does not mean the caller can modify the VM in any sense.
    /// Even a slight tampering of the VM can result in non-determinism.
    pub fn peek<F, G, W>(&mut self, vm_id: &VmId, mut f: F, mut g: G) -> Result<W, Error>
    where
        F: FnMut(&mut M) -> Result<W, Error>,
        G: FnMut(&Snapshot2<DataPieceId>, &SgData<DL>) -> Result<W, Error>,
    {
        if let Some((_, machine)) = self.instantiated.get_mut(vm_id) {
            return f(machine);
        }
        if let Some(snapshot) = self.suspended.get(vm_id) {
            return g(snapshot, &self.sg_data);
        }
        Err(Error::Unexpected(format!("VM {} does not exist!", vm_id)))
    }

    /// Add cycles to total cycles.
    fn consume_cycles(&mut self, cycles: Cycle) -> Result<(), Error> {
        match self
            .total_cycles
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |total_cycles| {
                total_cycles.checked_add(cycles)
            }) {
            Ok(_) => Ok(()),
            Err(_) => Err(Error::CyclesExceeded),
        }
    }

    /// Resume a previously suspended scheduler state
    pub fn resume(
        sg_data: SgData<DL>,
        syscall_generator: SyscallGenerator<DL, V, M::Inner>,
        syscall_context: V,
        full: FullSuspendedState,
    ) -> Self {
        let mut scheduler = Self {
            sg_data,
            syscall_generator,
            syscall_context,
            total_cycles: Arc::new(AtomicU64::new(full.total_cycles)),
            iteration_cycles: 0,
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
            message_box: Arc::new(Mutex::new(Vec::new())),
            terminated_vms: full.terminated_vms.into_iter().collect(),
        };
        scheduler
            .ensure_vms_instantiated(&full.instantiated_ids)
            .unwrap();
        // NOTE: suspending/resuming a scheduler is part of CKB's implementation
        // details. It is not part of execution consensue. We should not charge
        // cycles for them.
        scheduler.iteration_cycles = 0;
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
            // NOTE: suspending a scheduler is actually part of CKB's
            // internal execution logic, it does not belong to VM execution
            // consensus. We are not charging cycles for suspending
            // a VM in the process of suspending the whole scheduler.
            total_cycles: self.total_cycles.load(Ordering::Acquire),
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
    pub fn run(&mut self, mode: RunMode) -> Result<TerminatedResult, Error> {
        self.boot_root_vm_if_needed()?;

        let (pause, mut limit_cycles) = match mode {
            RunMode::LimitCycles(limit_cycles) => (Pause::new(), limit_cycles),
            RunMode::Pause(pause) => (pause, u64::MAX),
        };

        while !self.terminated() {
            limit_cycles = self.iterate_outer(&pause, limit_cycles)?.1;
        }

        self.terminated_result()
    }

    /// Public API that runs a single VM, processes all messages, then returns the
    /// executed VM ID(so caller can fetch later data). This can be used when more
    /// finer tweaks are required for a single VM.
    pub fn iterate(&mut self) -> Result<IterationResult, Error> {
        self.boot_root_vm_if_needed()?;

        if self.terminated() {
            return Ok(IterationResult {
                executed_vm: ROOT_VM_ID,
                terminated_status: Some(self.terminated_result()?),
            });
        }

        let (id, _) = self.iterate_outer(&Pause::new(), u64::MAX)?;
        let terminated_status = if self.terminated() {
            Some(self.terminated_result()?)
        } else {
            None
        };

        Ok(IterationResult {
            executed_vm: id,
            terminated_status,
        })
    }

    /// Returns the machine that needs to be executed in the current iterate.
    fn iterate_prepare_machine(&mut self) -> Result<(u64, &mut M), Error> {
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
        let (_context, machine) = self.ensure_get_instantiated(&vm_id_to_run)?;
        Ok((vm_id_to_run, machine))
    }

    /// Process machine execution results in the current iterate.
    fn iterate_process_results(
        &mut self,
        vm_id_to_run: u64,
        result: Result<i8, Error>,
    ) -> Result<(), Error> {
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
                            .inner_mut()
                            .memory_mut()
                            .store8(&Self::u64_to_reg(exit_code_addr), &Self::i8_to_reg(code))?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(SUCCESS));
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

    // This internal function is actually a wrapper over +iterate_inner+,
    // it is split into a different function, so cycle calculation will be
    // executed no matter what result +iterate_inner+ returns.
    #[inline]
    fn iterate_outer(
        &mut self,
        pause: &Pause,
        limit_cycles: Cycle,
    ) -> Result<(VmId, Cycle), Error> {
        assert_eq!(self.iteration_cycles, 0);
        let iterate_return = self.iterate_inner(pause.clone(), limit_cycles);
        self.consume_cycles(self.iteration_cycles)?;
        let remaining_cycles = limit_cycles
            .checked_sub(self.iteration_cycles)
            .ok_or(Error::CyclesExceeded)?;
        // Clear iteration cycles intentionally after each run
        self.iteration_cycles = 0;
        let id = iterate_return?;
        Ok((id, remaining_cycles))
    }

    // This is internal function that does the actual VM execution loop.
    // Here both pause signal and limit_cycles are provided so as to simplify
    // branches.
    fn iterate_inner(&mut self, pause: Pause, limit_cycles: Cycle) -> Result<VmId, Error> {
        // Execute the VM for real, consumed cycles in the virtual machine is
        // moved over to +iteration_cycles+, then we reset virtual machine's own
        // cycle count to zero.
        let (id, result, cycles) = {
            let (id, vm) = self.iterate_prepare_machine()?;
            vm.inner_mut().set_max_cycles(limit_cycles);
            vm.machine_mut().set_pause(pause);
            let result = vm.run();
            let cycles = vm.machine().cycles();
            vm.inner_mut().set_cycles(0);
            (id, result, cycles)
        };
        self.iteration_cycles = self
            .iteration_cycles
            .checked_add(cycles)
            .ok_or(Error::CyclesExceeded)?;
        self.iterate_process_results(id, result)?;
        Ok(id)
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
                        .inner_mut()
                        .add_cycles_no_checking(EXEC_LOAD_ELF_V2_CYCLES_BASE)?;
                    let old_cycles = old_machine.machine().cycles();
                    let max_cycles = old_machine.machine().max_cycles();
                    let program = {
                        let sc = old_context.snapshot2_context.lock().expect("lock");
                        sc.load_data(
                            &args.location.data_piece_id,
                            args.location.offset,
                            args.location.length,
                        )?
                        .0
                    };
                    let (context, mut new_machine) = self.create_dummy_vm(&vm_id)?;
                    new_machine.inner_mut().set_max_cycles(max_cycles);
                    new_machine.inner_mut().add_cycles_no_checking(old_cycles)?;
                    self.load_vm_program(
                        &context,
                        &mut new_machine,
                        &args.location,
                        program,
                        VmArgs::Reader {
                            vm_id,
                            argc: args.argc,
                            argv: args.argv,
                        },
                    )?;
                    // The insert operation removes the old vm instance and adds the new vm instance.
                    debug_assert!(self.instantiated.contains_key(&vm_id));
                    self.instantiated.insert(vm_id, (context, new_machine));
                }
                Message::Spawn(vm_id, args) => {
                    // All fds must belong to the correct owner
                    if args.fds.iter().any(|fd| self.fds.get(fd) != Some(&vm_id)) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(INVALID_FD));
                        continue;
                    }
                    if self.suspended.len() + self.instantiated.len() > MAX_VMS_COUNT as usize {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(MAX_VMS_SPAWNED));
                        continue;
                    }
                    let spawned_vm_id = self.boot_vm(
                        &args.location,
                        VmArgs::Reader {
                            vm_id,
                            argc: args.argc,
                            argv: args.argv,
                        },
                    )?;
                    // Move passed fds from spawner to spawnee
                    for fd in &args.fds {
                        self.fds.insert(*fd, spawned_vm_id);
                    }
                    // Here we keep the original version of file descriptors.
                    // If one fd is moved afterward, this inherited file descriptors doesn't change.
                    self.inherited_fd.insert(spawned_vm_id, args.fds.clone());

                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    machine.inner_mut().memory_mut().store64(
                        &Self::u64_to_reg(args.process_id_addr),
                        &Self::u64_to_reg(spawned_vm_id),
                    )?;
                    machine
                        .inner_mut()
                        .set_register(A0, Self::u8_to_reg(SUCCESS));
                }
                Message::Wait(vm_id, args) => {
                    if let Some(exit_code) = self.terminated_vms.get(&args.target_id).copied() {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine.inner_mut().memory_mut().store8(
                            &Self::u64_to_reg(args.exit_code_addr),
                            &Self::i8_to_reg(exit_code),
                        )?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(SUCCESS));
                        self.states.insert(vm_id, VmState::Runnable);
                        self.terminated_vms.retain(|id, _| id != &args.target_id);
                        continue;
                    }
                    if !self.states.contains_key(&args.target_id) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(WAIT_FAILURE));
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
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(MAX_FDS_CREATED));
                        continue;
                    }
                    let (p1, p2, slot) = Fd::create(self.next_fd_slot);
                    self.next_fd_slot = slot;
                    self.fds.insert(p1, vm_id);
                    self.fds.insert(p2, vm_id);
                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    machine
                        .inner_mut()
                        .memory_mut()
                        .store64(&Self::u64_to_reg(args.fd1_addr), &Self::u64_to_reg(p1.0))?;
                    machine
                        .inner_mut()
                        .memory_mut()
                        .store64(&Self::u64_to_reg(args.fd2_addr), &Self::u64_to_reg(p2.0))?;
                    machine
                        .inner_mut()
                        .set_register(A0, Self::u8_to_reg(SUCCESS));
                }
                Message::FdRead(vm_id, args) => {
                    if !(self.fds.contains_key(&args.fd) && (self.fds[&args.fd] == vm_id)) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(INVALID_FD));
                        continue;
                    }
                    if !self.fds.contains_key(&args.fd.other_fd()) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(OTHER_END_CLOSED));
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
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(INVALID_FD));
                        continue;
                    }
                    if !self.fds.contains_key(&args.fd.other_fd()) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(OTHER_END_CLOSED));
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
                        .inner_mut()
                        .memory_mut()
                        .load64(&Self::u64_to_reg(length_addr))?
                        .to_u64();
                    let real_length = inherited_fd.len() as u64;
                    let copy_length = u64::min(full_length, real_length);
                    for i in 0..copy_length {
                        let fd = inherited_fd[i as usize].0;
                        let addr = buffer_addr.checked_add(i * 8).ok_or(Error::MemOutOfBound)?;
                        machine
                            .inner_mut()
                            .memory_mut()
                            .store64(&Self::u64_to_reg(addr), &Self::u64_to_reg(fd))?;
                    }
                    machine.inner_mut().memory_mut().store64(
                        &Self::u64_to_reg(length_addr),
                        &Self::u64_to_reg(real_length),
                    )?;
                    machine
                        .inner_mut()
                        .set_register(A0, Self::u8_to_reg(SUCCESS));
                }
                Message::Close(vm_id, fd) => {
                    if self.fds.get(&fd) != Some(&vm_id) {
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(INVALID_FD));
                    } else {
                        self.fds.remove(&fd);
                        let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                        machine
                            .inner_mut()
                            .set_register(A0, Self::u8_to_reg(SUCCESS));
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
                    machine.inner_mut().memory_mut().store64(
                        &Self::u64_to_reg(length_addr),
                        &<M::Inner as CoreMachine>::REG::zero(),
                    )?;
                    machine
                        .inner_mut()
                        .set_register(A0, Self::u8_to_reg(SUCCESS));
                    self.states.insert(vm_id, VmState::Runnable);
                }
                VmState::WaitForWrite(WriteState {
                    consumed,
                    length_addr,
                    ..
                }) => {
                    let (_, machine) = self.ensure_get_instantiated(&vm_id)?;
                    machine
                        .inner_mut()
                        .memory_mut()
                        .store64(&Self::u64_to_reg(length_addr), &Self::u64_to_reg(consumed))?;
                    machine
                        .inner_mut()
                        .set_register(A0, Self::u8_to_reg(SUCCESS));
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
                    .inner_mut()
                    .add_cycles_no_checking(transferred_byte_cycles(copiable))?;
                let data = write_machine
                    .inner_mut()
                    .memory_mut()
                    .load_bytes(write_buffer_addr.wrapping_add(consumed), copiable)?;
                let (_, read_machine) = self
                    .instantiated
                    .get_mut(&read_vm_id)
                    .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
                read_machine
                    .inner_mut()
                    .add_cycles_no_checking(transferred_byte_cycles(copiable))?;
                read_machine
                    .inner_mut()
                    .memory_mut()
                    .store_bytes(read_buffer_addr, &data)?;
                // Read syscall terminates as soon as some data are filled
                read_machine.inner_mut().memory_mut().store64(
                    &Self::u64_to_reg(read_length_addr),
                    &Self::u64_to_reg(copiable),
                )?;
                read_machine
                    .inner_mut()
                    .set_register(A0, Self::u8_to_reg(SUCCESS));
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
                    write_machine.inner_mut().memory_mut().store64(
                        &Self::u64_to_reg(write_length_addr),
                        &Self::u64_to_reg(write_length),
                    )?;
                    write_machine
                        .inner_mut()
                        .set_register(A0, Self::u8_to_reg(SUCCESS));
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

    /// If current scheduler is terminated
    pub fn terminated(&self) -> bool {
        self.states
            .get(&ROOT_VM_ID)
            .map(|state| *state == VmState::Terminated)
            .unwrap_or(false)
    }

    fn terminated_result(&mut self) -> Result<TerminatedResult, Error> {
        assert!(self.terminated());

        let exit_code = {
            let root_vm = &self.ensure_get_instantiated(&ROOT_VM_ID)?.1;
            root_vm.machine().exit_code()
        };
        Ok(TerminatedResult {
            exit_code,
            consumed_cycles: self.consumed_cycles(),
        })
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

    /// Ensure corresponding VM is instantiated and return a mutable reference to it
    fn ensure_get_instantiated(&mut self, id: &VmId) -> Result<&mut (VmContext<DL>, M), Error> {
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
        self.iteration_cycles = self
            .iteration_cycles
            .checked_add(SPAWN_EXTRA_CYCLES_BASE)
            .ok_or(Error::CyclesExceeded)?;
        let (context, mut machine) = self.create_dummy_vm(id)?;
        {
            let mut sc = context.snapshot2_context.lock().expect("lock");
            sc.resume(machine.inner_mut(), snapshot)?;
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
        self.iteration_cycles = self
            .iteration_cycles
            .checked_add(SPAWN_EXTRA_CYCLES_BASE)
            .ok_or(Error::CyclesExceeded)?;
        let (context, machine) = self
            .instantiated
            .get_mut(id)
            .ok_or_else(|| Error::Unexpected("Unable to find VM Id".to_string()))?;
        let snapshot = {
            let sc = context.snapshot2_context.lock().expect("lock");
            sc.make_snapshot(machine.inner_mut())?
        };
        self.suspended.insert(*id, snapshot);
        self.instantiated.remove(id);
        Ok(())
    }

    fn boot_root_vm_if_needed(&mut self) -> Result<(), Error> {
        if self.states.is_empty() {
            // Booting phase, we will need to initialize the first VM.
            let program_id = self.sg_data.sg_info.program_data_piece_id.clone();
            assert_eq!(
                self.boot_vm(
                    &DataLocation {
                        data_piece_id: program_id,
                        offset: 0,
                        length: u64::MAX,
                    },
                    VmArgs::Vector(vec![]),
                )?,
                ROOT_VM_ID
            );
        }
        assert!(self.states.contains_key(&ROOT_VM_ID));

        Ok(())
    }

    /// Boot a vm by given program and args.
    fn boot_vm(&mut self, location: &DataLocation, args: VmArgs) -> Result<VmId, Error> {
        let id = self.next_vm_id;
        self.next_vm_id += 1;
        let (context, mut machine) = self.create_dummy_vm(&id)?;
        let (program, _) = {
            let sc = context.snapshot2_context.lock().expect("lock");
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
        context: &VmContext<DL>,
        machine: &mut M,
        location: &DataLocation,
        program: Bytes,
        args: VmArgs,
    ) -> Result<u64, Error> {
        let metadata = parse_elf::<u64>(&program, machine.inner_mut().version())?;
        let bytes = match args {
            VmArgs::Reader { vm_id, argc, argv } => {
                let (_, machine_from) = self.ensure_get_instantiated(&vm_id)?;
                let argc = Self::u64_to_reg(argc);
                let argv = Self::u64_to_reg(argv);
                let argv =
                    FlattenedArgsReader::new(machine_from.inner_mut().memory_mut(), argc, argv);
                machine.load_program_with_metadata(&program, &metadata, argv)?
            }
            VmArgs::Vector(data) => {
                machine.load_program_with_metadata(&program, &metadata, data.into_iter().map(Ok))?
            }
        };
        let mut sc = context.snapshot2_context.lock().expect("lock");
        sc.mark_program(
            machine.inner_mut(),
            &metadata,
            &location.data_piece_id,
            location.offset,
        )?;
        machine
            .inner_mut()
            .add_cycles_no_checking(transferred_byte_cycles(bytes))?;
        Ok(bytes)
    }

    // Create a new VM instance with syscalls attached
    fn create_dummy_vm(&self, id: &VmId) -> Result<(VmContext<DL>, M), Error> {
        let version = &self.sg_data.sg_info.script_version;
        let core_machine = M::Inner::new(
            version.vm_isa(),
            version.vm_version(),
            // We will update max_cycles for each machine when it gets a chance to run
            u64::MAX,
        );
        let vm_context = VmContext {
            base_cycles: Arc::clone(&self.total_cycles),
            message_box: Arc::clone(&self.message_box),
            snapshot2_context: Arc::new(Mutex::new(Snapshot2Context::new(self.sg_data.clone()))),
        };

        let machine_builder = DefaultMachineBuilder::new(core_machine)
            .instruction_cycle_func(Box::new(estimate_cycles));
        let machine_builder =
            (self.syscall_generator)(id, &self.sg_data, &vm_context, &self.syscall_context)
                .into_iter()
                .fold(machine_builder, |builder, syscall| builder.syscall(syscall));
        let default_machine = machine_builder.build();
        Ok((vm_context, M::new(default_machine)))
    }

    fn i8_to_reg(v: i8) -> <M::Inner as CoreMachine>::REG {
        <M::Inner as CoreMachine>::REG::from_i8(v)
    }

    fn u8_to_reg(v: u8) -> <M::Inner as CoreMachine>::REG {
        <M::Inner as CoreMachine>::REG::from_u8(v)
    }

    fn u64_to_reg(v: u64) -> <M::Inner as CoreMachine>::REG {
        <M::Inner as CoreMachine>::REG::from_u64(v)
    }
}
