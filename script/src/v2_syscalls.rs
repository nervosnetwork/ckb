use crate::{
    v2_types::{
        DataPieceId, Message, PipeArgs, PipeId, PipeIoArgs, SpawnArgs, TxData, VmId, WaitArgs,
    },
    ScriptVersion,
};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    bytes::Bytes,
    machine::SupportMachine,
    memory::{Memory, FLAG_EXECUTABLE, FLAG_FREEZED},
    registers::{A0, A1, A2, A3, A4, A5, A7},
    snapshot2::{DataSource, Snapshot2Context},
    syscalls::Syscalls,
    Error, Register,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MachineContext<
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
> {
    id: VmId,
    base_cycles: Arc<Mutex<u64>>,
    message_box: Arc<Mutex<Vec<Message>>>,
    snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    script_version: ScriptVersion,
}

impl<DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static>
    MachineContext<DL>
{
    pub fn new(
        id: VmId,
        message_box: Arc<Mutex<Vec<Message>>>,
        tx_data: TxData<DL>,
        script_version: ScriptVersion,
    ) -> Self {
        Self {
            id,
            base_cycles: Arc::new(Mutex::new(0)),
            message_box,
            snapshot2_context: Arc::new(Mutex::new(Snapshot2Context::new(tx_data))),
            script_version,
        }
    }

    pub fn snapshot2_context(&self) -> &Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>> {
        &self.snapshot2_context
    }

    pub fn base_cycles(&self) -> u64 {
        *self.base_cycles.lock().expect("lock")
    }

    pub fn set_base_cycles(&mut self, base_cycles: u64) {
        *self.base_cycles.lock().expect("lock") = base_cycles;
    }

    // The different architecture here requires a re-implementation on current
    // cycles syscall.
    fn current_cycles<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let cycles = self
            .base_cycles()
            .checked_add(machine.cycles())
            .ok_or(Error::CyclesOverflow)?;
        machine.set_register(A0, Mac::REG::from_u64(cycles));
        Ok(())
    }

    // Reimplementation of load_cell_data but keep tracks of pages that are copied from
    // surrounding transaction data. Those pages do not need to be added to snapshots.
    fn load_cell_data<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let index = machine.registers()[A3].to_u64();
        let source = machine.registers()[A4].to_u64();

        let data_piece_id = match DataPieceId::try_from((source, index)) {
            Ok(id) => id,
            Err(e) => {
                // Current implementation would throw an error immediately
                // for some source values, but return INDEX_OUT_OF_BOUND error
                // for other values. Here for simplicity, we would return
                // INDEX_OUT_OF_BOUND error in all cases. But the code might
                // differ to mimic current on-chain behavior
                println!("DataPieceId parsing error: {:?}", e);
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
        };

        let addr = machine.registers()[A0].to_u64();
        let size_addr = machine.registers()[A1].clone();
        let size = machine.memory_mut().load64(&size_addr)?.to_u64();
        let offset = machine.registers()[A2].to_u64();

        let mut sc = self.snapshot2_context().lock().expect("lock");
        let (wrote_size, full_size) =
            match sc.store_bytes(machine, addr, &data_piece_id, offset, size) {
                Ok(val) => val,
                Err(Error::External(m)) if m == "INDEX_OUT_OF_BOUND" => {
                    // This comes from TxData results in an out of bound error, to
                    // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                    machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                    return Ok(());
                }
                Err(e) => return Err(e),
            };

        machine
            .memory_mut()
            .store64(&size_addr, &Mac::REG::from_u64(full_size))?;
        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(())
    }

    // Reimplementation of load_cell_data_as_code but keep tracks of pages that are copied from
    // surrounding transaction data. Those pages do not need to be added to snapshots.
    //
    // Different from load_cell_data, this method showcases advanced usage of Snapshot2, where
    // one manually does the actual memory copying, then calls track_pages method to setup metadata
    // used by Snapshot2. It does not rely on higher level methods provided by Snapshot2.
    fn load_cell_data_as_code<Mac: SupportMachine>(
        &mut self,
        machine: &mut Mac,
    ) -> Result<(), Error> {
        let addr = machine.registers()[A0].to_u64();
        let memory_size = machine.registers()[A1].to_u64();
        let content_offset = machine.registers()[A2].to_u64();
        let content_size = machine.registers()[A3].to_u64();

        let index = machine.registers()[A4].to_u64();
        let source = machine.registers()[A5].to_u64();

        let data_piece_id = match DataPieceId::try_from((source, index)) {
            Ok(id) => id,
            Err(e) => {
                // Current implementation would throw an error immediately
                // for some source values, but return INDEX_OUT_OF_BOUND error
                // for other values. Here for simplicity, we would return
                // INDEX_OUT_OF_BOUND error in all cases. But the code might
                // differ to mimic current on-chain behavior
                println!("DataPieceId parsing error: {:?}", e);
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
        };

        let mut sc = self.snapshot2_context().lock().expect("lock");
        // We are using 0..u64::max_value() to fetch full cell, there is
        // also no need to keep the full length value. Since cell's length
        // is already full length.
        let (cell, _) = match sc
            .data_source()
            .load_data(&data_piece_id, 0, u64::max_value())
        {
            Ok(val) => val,
            Err(Error::External(m)) if m == "INDEX_OUT_OF_BOUND" => {
                // This comes from TxData results in an out of bound error, to
                // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        let content_end = content_offset
            .checked_add(content_size)
            .ok_or(Error::MemOutOfBound)?;
        if content_offset >= cell.len() as u64
            || content_end > cell.len() as u64
            || content_size > memory_size
        {
            machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
            return Ok(());
        }

        machine.memory_mut().init_pages(
            addr,
            memory_size,
            FLAG_EXECUTABLE | FLAG_FREEZED,
            Some(cell.slice((content_offset as usize)..(content_end as usize))),
            0,
        )?;
        sc.track_pages(machine, addr, memory_size, &data_piece_id, content_offset)?;

        machine.add_cycles_no_checking(transferred_byte_cycles(memory_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(())
    }

    // Reimplementing debug syscall for printing debug messages
    fn debug<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let mut addr = machine.registers()[A0].to_u64();
        let mut buffer = Vec::new();

        loop {
            let byte = machine
                .memory_mut()
                .load8(&Mac::REG::from_u64(addr))?
                .to_u8();
            if byte == 0 {
                break;
            }
            buffer.push(byte);
            addr += 1;
        }

        machine.add_cycles_no_checking(transferred_byte_cycles(buffer.len() as u64))?;
        let s = String::from_utf8(buffer)
            .map_err(|e| Error::External(format!("String from buffer {e:?}")))?;
        println!("VM {}: {}", self.id, s);

        Ok(())
    }

    // New, concurrent spawn implementation
    fn spawn<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let index = machine.registers()[A0].to_u64();
        let source = machine.registers()[A1].to_u64();
        let place = machine.registers()[A2].to_u64(); // TODO: support reading data from witness

        let data_piece_id = match DataPieceId::try_from((source, index)) {
            Ok(id) => id,
            Err(e) => {
                // Current implementation would throw an error immediately
                // for some source values, but return INDEX_OUT_OF_BOUND error
                // for other values. Here for simplicity, we would return
                // INDEX_OUT_OF_BOUND error in all cases. But the code might
                // differ to mimic current on-chain behavior
                println!("DataPieceId parsing error: {:?}", e);
                machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                return Ok(());
            }
        };

        let bounds = machine.registers()[A3].to_u64();
        let offset = bounds >> 32;
        let length = bounds as u32 as u64;

        let spgs_addr = machine.registers()[A4].to_u64();
        let argc_addr = spgs_addr;
        let argc = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(argc_addr))?
            .to_u64();
        let mut argv_addr = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(spgs_addr.wrapping_add(8)))?
            .to_u64();
        let mut argv = Vec::new();
        for _ in 0..argc {
            let target_addr = machine
                .memory_mut()
                .load64(&Mac::REG::from_u64(argv_addr))?
                .to_u64();
            let cstr = load_c_string(machine, target_addr)?;
            argv.push(cstr);
            argv_addr = argv_addr.wrapping_add(8);
        }

        let (process_id_addr, pipes) = {
            let process_id_addr_addr = spgs_addr.wrapping_add(16);
            let process_id_addr = machine
                .memory_mut()
                .load64(&Mac::REG::from_u64(process_id_addr_addr))?
                .to_u64();
            let pipes_addr_addr = spgs_addr.wrapping_add(24);
            let mut pipes_addr = machine
                .memory_mut()
                .load64(&Mac::REG::from_u64(pipes_addr_addr))?
                .to_u64();

            let mut pipes = vec![];
            if pipes_addr != 0 {
                loop {
                    let pipe = machine
                        .memory_mut()
                        .load64(&Mac::REG::from_u64(pipes_addr))?
                        .to_u64();
                    if pipe == 0 {
                        break;
                    }
                    pipes.push(PipeId(pipe));
                    pipes_addr += 8;
                }
            }
            (process_id_addr, pipes)
        };

        // We are fetching the actual cell here for some in-place validation
        {
            let sc = self.snapshot2_context().lock().expect("lock");
            let (_, full_length) = match sc.data_source().load_data(&data_piece_id, 0, 0) {
                Ok(val) => val,
                Err(Error::External(m)) if m == "INDEX_OUT_OF_BOUND" => {
                    // This comes from TxData results in an out of bound error, to
                    // mimic current behavior, we would return INDEX_OUT_OF_BOUND error.
                    machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            if offset >= full_length {
                machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
                return Ok(());
            }
            if length > 0 {
                let end = offset.checked_add(length).ok_or(Error::MemOutOfBound)?;
                if end > full_length {
                    machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
                    return Ok(());
                }
            }
        }
        // TODO: update spawn base cycles
        machine.add_cycles_no_checking(100_000)?;
        self.message_box.lock().expect("lock").push(Message::Spawn(
            self.id,
            SpawnArgs {
                data_piece_id,
                offset,
                length,
                argv,
                pipes,
                process_id_addr,
            },
        ));

        // At this point, all execution has been finished, and it is expected
        // to return Ok(()) denoting success. However we want spawn to yield
        // its control back to scheduler, so a runnable VM with a higher ID can
        // start its execution first. That's why we actually return a yield error
        // here.
        Err(Error::External("YIELD".to_string()))
    }

    // Join syscall blocks till the specified VM finishes execution, then
    // returns with its exit code
    fn wait<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let target_id = machine.registers()[A0].to_u64();
        let exit_code_addr = machine.registers()[A1].to_u64();

        // TODO: charge cycles
        self.message_box.lock().expect("lock").push(Message::Wait(
            self.id,
            WaitArgs {
                target_id,
                exit_code_addr,
            },
        ));

        // Like spawn, join yields control upon success
        Err(Error::External("YIELD".to_string()))
    }

    // Fetch current instance ID
    fn process_id<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        // TODO: charge cycles
        machine.set_register(A0, Mac::REG::from_u64(self.id));
        Ok(())
    }

    // Create a pair of pipes
    fn pipe<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let pipe1_addr = machine.registers()[A0].to_u64();
        let pipe2_addr = pipe1_addr.wrapping_add(8);
        // TODO: charge cycles
        self.message_box.lock().expect("lock").push(Message::Pipe(
            self.id,
            PipeArgs {
                pipe1_addr,
                pipe2_addr,
            },
        ));

        Err(Error::External("YIELD".to_string()))
    }

    // Write to pipe
    fn pipe_write<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let pipe = PipeId(machine.registers()[A0].to_u64());
        let buffer_addr = machine.registers()[A1].to_u64();
        let length_addr = machine.registers()[A2].to_u64();
        let length = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(length_addr))?
            .to_u64();

        // We can only do basic checks here, when the message is actually processed,
        // more complete checks will be performed.
        // We will also leave to the actual write operation to test memory permissions.
        if !pipe.is_write() {
            machine.set_register(A0, Mac::REG::from_u8(INVALID_PIPE));
            return Ok(());
        }

        // TODO: charge cycles
        self.message_box
            .lock()
            .expect("lock")
            .push(Message::PipeWrite(
                self.id,
                PipeIoArgs {
                    pipe,
                    length,
                    buffer_addr,
                    length_addr,
                },
            ));

        // A0 will be updated once the write operation is fulfilled
        Err(Error::External("YIELD".to_string()))
    }

    // Read from pipe
    fn pipe_read<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let pipe = PipeId(machine.registers()[A0].to_u64());
        let buffer_addr = machine.registers()[A1].to_u64();
        let length_addr = machine.registers()[A2].to_u64();
        let length = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(length_addr))?
            .to_u64();

        // We can only do basic checks here, when the message is actually processed,
        // more complete checks will be performed.
        // We will also leave to the actual write operation to test memory permissions.
        if !pipe.is_read() {
            machine.set_register(A0, Mac::REG::from_u8(INVALID_PIPE));
            return Ok(());
        }

        // TODO: charge cycles
        self.message_box
            .lock()
            .expect("lock")
            .push(Message::PipeRead(
                self.id,
                PipeIoArgs {
                    pipe,
                    length,
                    buffer_addr,
                    length_addr,
                },
            ));

        // A0 will be updated once the read operation is fulfilled
        Err(Error::External("YIELD".to_string()))
    }

    fn inherited_file_descriptors<Mac: SupportMachine>(
        &mut self,
        machine: &mut Mac,
    ) -> Result<(), Error> {
        let buffer_addr = machine.registers()[A0].to_u64();
        let length_addr = machine.registers()[A1].to_u64();
        self.message_box
            .lock()
            .expect("lock")
            .push(Message::InheritedFileDescriptor(
                self.id,
                PipeIoArgs {
                    pipe: PipeId(0),
                    length: 0,
                    buffer_addr,
                    length_addr,
                },
            ));
        Err(Error::External("YIELD".to_string()))
    }

    fn close<Mac: SupportMachine>(&mut self, machine: &mut Mac) -> Result<(), Error> {
        let pipe = PipeId(machine.registers()[A0].to_u64());
        self.message_box
            .lock()
            .expect("lock")
            .push(Message::Close(self.id, pipe));
        Err(Error::External("YIELD".to_string()))
    }
}

impl<
        Mac: SupportMachine,
        DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
    > Syscalls<Mac> for MachineContext<DL>
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), Error> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, Error> {
        let code = machine.registers()[A7].to_u64();
        match code {
            2042 => {
                if self.script_version >= ScriptVersion::V1 {
                    self.current_cycles(machine)
                } else {
                    return Ok(false);
                }
            }
            2091 => self.load_cell_data_as_code(machine),
            2092 => self.load_cell_data(machine),
            2177 => self.debug(machine),
            // The syscall numbers here are picked intentionally to be different
            // than currently assigned syscall numbers for spawn calls
            2601 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.spawn(machine)
                } else {
                    return Ok(false);
                }
            }
            2602 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.wait(machine)
                } else {
                    return Ok(false);
                }
            }
            2603 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.process_id(machine)
                } else {
                    return Ok(false);
                }
            }
            2604 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.pipe(machine)
                } else {
                    return Ok(false);
                }
            }
            2605 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.pipe_write(machine)
                } else {
                    return Ok(false);
                }
            }
            2606 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.pipe_read(machine)
                } else {
                    return Ok(false);
                }
            }
            2607 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.inherited_file_descriptors(machine)
                } else {
                    return Ok(false);
                }
            }
            2608 => {
                if self.script_version >= ScriptVersion::V2 {
                    self.close(machine)
                } else {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }?;
        Ok(true)
    }
}

// Below are all simple utilities copied over from ckb-script package to
// ease the implementation.

/// How many bytes can transfer when VM costs one cycle.
// 0.25 cycles per byte
const BYTES_PER_CYCLE: u64 = 4;

/// Calculates how many cycles spent to load the specified number of bytes.
pub(crate) fn transferred_byte_cycles(bytes: u64) -> u64 {
    // Compiler will optimize the divisin here to shifts.
    (bytes + BYTES_PER_CYCLE - 1) / BYTES_PER_CYCLE
}

pub(crate) const SUCCESS: u8 = 0;
pub(crate) const INDEX_OUT_OF_BOUND: u8 = 1;
pub(crate) const SLICE_OUT_OF_BOUND: u8 = 3;
pub(crate) const WAIT_FAILURE: u8 = 5;
pub(crate) const INVALID_PIPE: u8 = 6;
pub(crate) const OTHER_END_CLOSED: u8 = 7;

fn load_c_string<Mac: SupportMachine>(machine: &mut Mac, addr: u64) -> Result<Bytes, Error> {
    let mut buffer = Vec::new();
    let mut addr = addr;

    loop {
        let byte = machine
            .memory_mut()
            .load8(&Mac::REG::from_u64(addr))?
            .to_u8();
        if byte == 0 {
            break;
        }
        buffer.push(byte);
        addr += 1;
    }

    Ok(Bytes::from(buffer))
}
