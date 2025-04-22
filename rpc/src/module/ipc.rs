use crate::error::RPCError;
use async_trait::async_trait;
use ckb_chain_spec::consensus::TYPE_ID_CODE_HASH;
use ckb_ipc::{Packet, Pipe, RequestPacket, ResponsePacket};
use ckb_jsonrpc_types::{
    IndexerOrder, IndexerScriptType, IndexerSearchKey, IpcEnv, IpcPayloadFormat, IpcRequest,
    IpcResponse, IpcScriptLocator, JsonBytes, Script, ScriptGroupType, ScriptHashType, Uint64,
};
use ckb_script::{
    CLOSE, INHERITED_FD, READ, TransactionScriptsVerifier, TxData, TxVerifyEnv, WRITE,
    generate_ckb_syscalls,
    types::{DebugPrinter, FIRST_FD_SLOT, FIRST_VM_ID, Machine, SgData, VmContext, VmId},
};
use ckb_script::{ChunkCommand, RunMode};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::{
    H256,
    bytes::Bytes,
    core::{
        Capacity, DepType, TransactionBuilder,
        cell::{CellMeta, CellProvider, CellStatus, HeaderChecker, resolve_transaction},
        error::OutPointError,
    },
    packed,
    prelude::{Builder, Entity, IntoTransactionView, Pack},
};
use ckb_vm::{
    Memory, Register, SupportMachine, Syscalls,
    registers::{A0, A1, A2, A7},
};
use faster_hex::{hex_decode, hex_string};
use jsonrpc_core::{Result, serde_from_str};
use jsonrpc_utils::rpc;
use serde_json::Value;
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};

use super::IndexerRpcImpl;

struct ScopeCall<F: FnMut()> {
    c: F,
}

impl<F: FnMut()> Drop for ScopeCall<F> {
    fn drop(&mut self) {
        (self.c)();
    }
}

const READER_FD: u64 = FIRST_FD_SLOT - 2;
const WRITER_FD: u64 = FIRST_FD_SLOT - 1;

#[derive(Clone)]
struct Resource {
    cell: HashMap<packed::OutPoint, CellStatus>,
    shared: Shared,
}

impl Resource {
    fn new(shared: Shared) -> Self {
        Self {
            cell: HashMap::new(),
            shared,
        }
    }
}

impl CellProvider for Resource {
    fn cell(&self, out_point: &packed::OutPoint, _: bool) -> CellStatus {
        match self.cell.get(out_point) {
            Some(data) => data.clone(),
            None => self.shared.snapshot().cell(out_point, true),
        }
    }
}

impl CellDataProvider for Resource {
    fn get_cell_data(&self, out_point: &packed::OutPoint) -> Option<Bytes> {
        if let CellStatus::Live(cell_meta) = self.cell(out_point, true) {
            cell_meta.mem_cell_data
        } else {
            None
        }
    }

    fn get_cell_data_hash(&self, out_point: &packed::OutPoint) -> Option<packed::Byte32> {
        if let CellStatus::Live(cell_meta) = self.cell(out_point, true) {
            cell_meta.mem_cell_data_hash
        } else {
            None
        }
    }
}

impl ExtensionProvider for Resource {
    fn get_block_extension(
        &self,
        hash: &ckb_types::packed::Byte32,
    ) -> Option<ckb_types::packed::Bytes> {
        self.shared.snapshot().get_block_extension(hash)
    }
}

impl HeaderChecker for Resource {
    fn check_valid(&self, block_hash: &packed::Byte32) -> std::result::Result<(), OutPointError> {
        self.shared.snapshot().check_valid(block_hash)
    }
}

impl HeaderProvider for Resource {
    fn get_header(&self, hash: &ckb_types::packed::Byte32) -> Option<ckb_types::core::HeaderView> {
        self.shared.snapshot().get_header(hash)
    }
}

#[derive(Clone)]
struct PipesCtx {
    nr: Arc<Mutex<Pipe>>, // Native Reader
    nw: Arc<Mutex<Pipe>>, // Native Writer
    vr: Arc<Mutex<Pipe>>, // Vm Reader
    vw: Arc<Mutex<Pipe>>, // Vm Writer
}

impl Default for PipesCtx {
    fn default() -> Self {
        let (nr, vw) = Pipe::new_pair();
        let (vr, nw) = Pipe::new_pair();
        Self {
            nr: Arc::new(Mutex::new(nr)),
            vw: Arc::new(Mutex::new(vw)),
            vr: Arc::new(Mutex::new(vr)),
            nw: Arc::new(Mutex::new(nw)),
        }
    }
}

impl PipesCtx {
    fn close(&self) {
        let _ = self.vw.lock().map(|mut e| e.close());
        let _ = self.vr.lock().map(|mut e| e.close());
        let _ = self.nw.lock().map(|mut e| e.close());
        let _ = self.nr.lock().map(|mut e| e.close());
    }
}

struct SyscallRead {
    id: VmId,
    pipectx: PipesCtx,
}

impl SyscallRead {
    pub fn new(id: &VmId, pipectx: &PipesCtx) -> Self {
        Self {
            id: *id,
            pipectx: pipectx.clone(),
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for SyscallRead {
    fn initialize(&mut self, _machine: &mut Mac) -> std::result::Result<(), ckb_vm::error::Error> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> std::result::Result<bool, ckb_vm::error::Error> {
        let code = &machine.registers()[A7];
        if code.to_u64() != READ {
            return Ok(false);
        }
        if self.id != FIRST_VM_ID {
            return Ok(false);
        }
        let fd = machine.registers()[A0].to_u64();
        if fd != READER_FD {
            return Ok(false);
        }
        let buffer_addr = machine.registers()[A1].clone();
        let length_addr = machine.registers()[A2].clone();
        let length = machine.memory_mut().load64(&length_addr)?.to_u64() as usize;
        // Limit the max of the lenght to 32k.
        let mut buf = vec![0; min(length, 32 * 1024)];
        let actual = self
            .pipectx
            .vr
            .lock()
            .map_err(|e| ckb_vm::error::Error::Unexpected(e.to_string()))?
            .read(&mut buf)
            .map_err(|e| ckb_vm::error::Error::Unexpected(e.to_string()))?;
        machine
            .memory_mut()
            .store_bytes(buffer_addr.to_u64(), &buf[..actual])?;
        machine
            .memory_mut()
            .store64(&length_addr, &Mac::REG::from_u64(actual as u64))?;
        machine.set_register(A0, Mac::REG::from_u8(0));
        Ok(true)
    }
}

struct SyscallWrite {
    id: VmId,
    pipesctx: PipesCtx,
}

impl SyscallWrite {
    pub fn new(id: &VmId, pipesctx: &PipesCtx) -> Self {
        Self {
            id: *id,
            pipesctx: pipesctx.clone(),
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for SyscallWrite {
    fn initialize(&mut self, _machine: &mut Mac) -> std::result::Result<(), ckb_vm::error::Error> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> std::result::Result<bool, ckb_vm::error::Error> {
        let code = &machine.registers()[A7];
        if code.to_u64() != WRITE {
            return Ok(false);
        }
        if self.id != FIRST_VM_ID {
            return Ok(false);
        }
        let fd = machine.registers()[A0].to_u64();
        if fd != WRITER_FD {
            return Ok(false);
        }
        let buffer_addr = machine.registers()[A1].clone();
        let length_addr = machine.registers()[A2].clone();
        let length = machine.memory_mut().load64(&length_addr)?.to_u64();
        // Skip zero-length writes to prevent false EOF signals.
        // The ckb-script scheduler allows zero-length data transfers, which could be misinterpreted as EOF by
        // higher-level APIs.
        if length == 0 {
            machine.set_register(A0, Mac::REG::from_u8(0));
            return Ok(true);
        }
        let data = machine
            .memory_mut()
            .load_bytes(buffer_addr.to_u64(), length)?;
        // The pipe write can't write partial data so we don't need to check result length.
        self.pipesctx
            .vw
            .lock()
            .map_err(|e| ckb_vm::error::Error::Unexpected(e.to_string()))?
            .write(&data)
            .map_err(|e| ckb_vm::error::Error::Unexpected(e.to_string()))?;
        machine
            .memory_mut()
            .store64(&length_addr, &Mac::REG::from_u64(data.len() as u64))?;
        machine.set_register(A0, Mac::REG::from_u8(0));
        Ok(true)
    }
}

pub struct SyscallInheritedFd {
    id: VmId,
}

impl SyscallInheritedFd {
    pub fn new(id: &VmId) -> Self {
        Self { id: *id }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for SyscallInheritedFd {
    fn initialize(&mut self, _machine: &mut Mac) -> std::result::Result<(), ckb_vm::error::Error> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> std::result::Result<bool, ckb_vm::error::Error> {
        let code = &machine.registers()[A7];
        if code.to_u64() != INHERITED_FD {
            return Ok(false);
        }
        if self.id != FIRST_VM_ID {
            return Ok(false);
        }
        let buffer_addr = machine.registers()[A0].clone();
        let length_addr = machine.registers()[A1].clone();
        let length = machine.memory_mut().load64(&length_addr)?;
        if length.to_u64() < 2 {
            return Err(ckb_vm::error::Error::Unexpected(String::from(
                "Length of inherited fd is less than 2",
            )));
        }
        let mut inherited_fd = [0u8; 16];
        inherited_fd[0x00..0x08].copy_from_slice(&READER_FD.to_le_bytes());
        inherited_fd[0x08..0x10].copy_from_slice(&WRITER_FD.to_le_bytes());
        machine
            .memory_mut()
            .store_bytes(buffer_addr.to_u64(), &inherited_fd[..])?;
        machine
            .memory_mut()
            .store64(&length_addr, &Mac::REG::from_u64(2))?;
        machine.set_register(A0, Mac::REG::from_u8(0));
        Ok(true)
    }
}

pub struct SyscallClose {
    id: VmId,
    pipesctx: PipesCtx,
}

impl SyscallClose {
    fn new(id: &VmId, pipesctx: &PipesCtx) -> Self {
        Self {
            id: *id,
            pipesctx: pipesctx.clone(),
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for SyscallClose {
    fn initialize(&mut self, _machine: &mut Mac) -> std::result::Result<(), ckb_vm::error::Error> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> std::result::Result<bool, ckb_vm::error::Error> {
        let code = &machine.registers()[A7];
        if code.to_u64() != CLOSE {
            return Ok(false);
        }
        if self.id != FIRST_VM_ID {
            return Ok(false);
        }
        let fd = machine.registers()[A0].to_u64();
        match fd {
            READER_FD => {
                self.pipesctx
                    .vr
                    .lock()
                    .map_err(|e| ckb_vm::error::Error::Unexpected(e.to_string()))?
                    .close();
                machine.set_register(A0, Mac::REG::from_u8(0));
                Ok(true)
            }
            WRITER_FD => {
                self.pipesctx
                    .vw
                    .lock()
                    .map_err(|e| ckb_vm::error::Error::Unexpected(e.to_string()))?
                    .close();
                machine.set_register(A0, Mac::REG::from_u8(0));
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

fn overload_ckb_syscalls<DL, M>(
    vm_id: &VmId,
    sg_data: &SgData<DL>,
    vm_context: &VmContext<DL>,
    pipesctx: &PipesCtx,
) -> Vec<Box<(dyn Syscalls<M>)>>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
    M: SupportMachine,
{
    let debug_printer: DebugPrinter = Arc::new(|_: &packed::Byte32, _: &str| {});

    let mut raw = generate_ckb_syscalls(vm_id, sg_data, vm_context, &debug_printer);
    raw.insert(0, Box::new(SyscallClose::new(vm_id, pipesctx)));
    raw.insert(0, Box::new(SyscallInheritedFd::new(vm_id)));
    raw.insert(0, Box::new(SyscallRead::new(vm_id, pipesctx)));
    raw.insert(0, Box::new(SyscallWrite::new(vm_id, pipesctx)));
    raw
}

/// RPC Module IPC.
#[rpc(openrpc)]
#[async_trait]
pub trait IpcRpc {
    /// Call the ipc method in the script.
    ///
    /// ## Params
    ///
    /// * script_locator - Used to locate the ipc script. You only need to provide one of the following two.
    ///     - out_point - Reference to the cell by transaction hash and output index.
    ///     - type_id_args - Reference to the cell by type id type script args.
    /// * req - Request.
    ///     - version - IPC protocol version. Default is 0.
    ///     - method_id - IPC protocol method id. Default is 0.
    ///     - payload_format - The format of payload.
    ///     - payload - Payload.
    /// * env: The transaction environment that the ipc script depends on. The script specified by script_group_type + script_hash will be executed, but the script binary will be replaced by the ipc script.
    ///     - tx - The transaction.
    ///     - script_group_type - Script group type.
    ///     - script_hash - Script hash.
    ///
    /// ## Returns
    ///
    /// * version - IPC protocol version. Default is 0.
    /// * error_code - IPC error code.
    /// * payload_format - The format of payload.
    /// * payload - Payload.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "ipc_call",
    ///     "params": [
    ///         {
    ///             "out_point": {
    ///                 "tx_hash": "0x4e75e1d4dddc0efe5e028168f627e3af436f1002b61038d695b24aa8441ffaf5",
    ///                 "index": "0x0"
    ///             }
    ///         },
    ///         {
    ///             "version": "0x0",
    ///             "method_id": "0x0",
    ///             "payload_format": "json",
    ///             "payload": {
    ///                 "SyscallLoadScript": {}
    ///             }
    ///         },
    ///         {
    ///             "tx": {
    ///                 "version": "0x0",
    ///                 "cell_deps": [
    ///                     {
    ///                         "out_point": {
    ///                             "tx_hash": "0xe7db03de9c534cb63d951f5378d5a4fbf43a2ebf5e48cb626562d188a3697772",
    ///                             "index": "0x0"
    ///                         },
    ///                         "dep_type": "dep_group"
    ///                     }
    ///                 ],
    ///                 "header_deps": [],
    ///                 "inputs": [
    ///                     {
    ///                         "since": "0x0",
    ///                         "previous_output": {
    ///                             "tx_hash": "0x953d1a4b95cefca9588b6d0bf7e97084bb51010ce87d1bcbe65c0ccd621781b8",
    ///                             "index": "0x0"
    ///                         }
    ///                     }
    ///                 ],
    ///                 "outputs": [],
    ///                 "outputs_data": [],
    ///                 "witnesses": [
    ///                     "0x55000000100000005500000055000000410000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    ///                 ]
    ///             },
    ///             "script_group_type": "lock",
    ///             "script_hash": "0x0b1bae4beaf456349c63c3ce67491fc75a1276d7f9eedd7ea84d6a77f9f3f5f7"
    ///         }
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///         "error_code": "0x0",
    ///         "payload": {
    ///             "SyscallLoadScript": "0x490000001000000030000000310000009bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8011400000075178f34549c5fe9cd1a0c57aebd01e7ddf9249e"
    ///         },
    ///         "payload_format": "json",
    ///         "version": "0x0"
    ///     }
    ///     "id": 2
    /// }
    /// ```
    #[rpc(name = "ipc_call")]
    fn ipc_call(
        &self,
        script_locator: IpcScriptLocator,
        req: IpcRequest,
        env: Option<IpcEnv>,
    ) -> Result<IpcResponse>;
}

#[derive(Clone)]
pub(crate) struct IpcRpcImpl {
    shared: Shared,
    indexer_rpc_impl: Option<IndexerRpcImpl>,
}

impl IpcRpcImpl {
    pub fn new(shared: Shared, indexer_rpc_impl: Option<IndexerRpcImpl>) -> Self {
        Self {
            shared,
            indexer_rpc_impl,
        }
    }

    fn ipc_env_none(&self, dl: &mut Resource, ipc_script_data: Bytes) -> Result<IpcEnv> {
        let cell_meta_ipc_script_data = ipc_script_data;
        let cell_meta_ipc_script = CellMeta {
            cell_output: packed::CellOutput::new_builder()
                .type_(
                    packed::ScriptOpt::new_builder()
                        .set(Some(
                            packed::Script::new_builder()
                                .code_hash(packed::Byte32::new_unchecked(
                                    TYPE_ID_CODE_HASH.as_bytes().into(),
                                ))
                                .hash_type(ckb_types::core::ScriptHashType::Type.into())
                                .args(Bytes::copy_from_slice(&[0u8; 32]).pack())
                                .build(),
                        ))
                        .build(),
                )
                .build(),
            out_point: packed::OutPoint::new(
                packed::Byte32::new_unchecked(vec![0x00; 32].into()),
                0,
            ),
            data_bytes: cell_meta_ipc_script_data.len() as u64,
            mem_cell_data: Some(cell_meta_ipc_script_data.clone()),
            mem_cell_data_hash: Some(packed::Byte32::new_unchecked(
                ckb_hash::blake2b_256(&cell_meta_ipc_script_data)
                    .to_vec()
                    .into(),
            )),
            ..Default::default()
        };
        let cell_meta_i = CellMeta {
            cell_output: packed::CellOutput::new_builder()
                .lock(
                    packed::Script::new_builder()
                        .code_hash(
                            cell_meta_ipc_script
                                .cell_output
                                .type_()
                                .to_opt()
                                .ok_or_else(|| {
                                    RPCError::custom_with_error(RPCError::IPC, "Unexpected")
                                })?
                                .calc_script_hash(),
                        )
                        .hash_type(ckb_types::core::ScriptHashType::Type.into())
                        .build(),
                )
                .build_exact_capacity(Capacity::zero())
                .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?,
            out_point: packed::OutPoint::new(packed::Byte32::zero(), 1),
            ..Default::default()
        };

        dl.cell.insert(
            cell_meta_ipc_script.out_point.clone(),
            CellStatus::Live(cell_meta_ipc_script.clone()),
        );
        dl.cell.insert(
            cell_meta_i.out_point.clone(),
            CellStatus::Live(cell_meta_i.clone()),
        );

        let tx = TransactionBuilder::default();
        let tx = tx.cell_dep(
            packed::CellDep::new_builder()
                .out_point(cell_meta_ipc_script.out_point)
                .dep_type(DepType::Code.into())
                .build(),
        );
        let tx = tx.input(packed::CellInput::new(cell_meta_i.out_point, 0));
        let tx = tx.build();

        Ok(IpcEnv {
            tx: tx.data().into(),
            script_group_type: ScriptGroupType::Lock,
            script_hash: {
                let mut buf = [0; 32];
                buf.copy_from_slice(cell_meta_i.cell_output.lock().calc_script_hash().as_slice());
                H256::from(buf)
            },
        })
    }

    fn ipc_env_some(
        &self,
        dl: &mut Resource,
        ipc_script_data: Bytes,
        env: IpcEnv,
    ) -> Result<IpcEnv> {
        let tx = packed::Transaction::from(env.tx.clone()).into_view();
        let tx = resolve_transaction(tx, &mut HashSet::new(), dl, dl)
            .map_err(|e| RPCError::custom_with_error(RPCError::TransactionFailedToResolve, e))?;
        let snapshot = self.shared.cloned_snapshot();
        let data = Arc::new(TxData::new(
            Arc::new(tx.clone()),
            dl.clone(),
            snapshot.cloned_consensus(),
            Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header())),
        ));
        let sg = data
            .find_script_group(
                match env.script_group_type {
                    ScriptGroupType::Lock => ckb_script::ScriptGroupType::Lock,
                    ScriptGroupType::Type => ckb_script::ScriptGroupType::Type,
                },
                &env.script_hash.pack(),
            )
            .ok_or_else(|| {
                RPCError::custom_with_error(
                    RPCError::IPC,
                    format!(
                        "{:?}",
                        ckb_script::ScriptError::ScriptNotFound(env.script_hash.pack())
                    ),
                )
            })?;
        let ci = data
            .extract_referenced_dep_index(&sg.script)
            .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?;
        let mut meta = tx.resolved_cell_deps[ci].clone();
        meta.mem_cell_data = Some(ipc_script_data);
        dl.cell
            .insert(meta.out_point.clone(), CellStatus::Live(meta));
        return Ok(env);
    }

    fn ipc_env(
        &self,
        dl: &mut Resource,
        ipc_script_data: Bytes,
        env: Option<IpcEnv>,
    ) -> Result<IpcEnv> {
        // If the user doesn't pass in an env, we will construct a default env.
        match env {
            Some(val) => self.ipc_env_some(dl, ipc_script_data, val),
            None => self.ipc_env_none(dl, ipc_script_data),
        }
    }

    fn ipc_script_data_by_out_point(&self, script_locator: IpcScriptLocator) -> Result<Bytes> {
        let out_point = script_locator
            .out_point
            .ok_or_else(|| RPCError::custom_with_error(RPCError::IPC, "Unexpected"))?;
        let out_point: packed::OutPoint = out_point.into();
        if let Some((data, _)) = self.shared.snapshot().get_cell_data(&out_point) {
            Ok(data)
        } else {
            Err(RPCError::custom(
                RPCError::IPC,
                format!("Get out point failed: {}", out_point),
            ))
        }
    }

    fn ipc_script_data_by_type_id_args(&self, script_locator: IpcScriptLocator) -> Result<Bytes> {
        match &self.indexer_rpc_impl {
            Some(data) => {
                let type_id_args = script_locator
                    .type_id_args
                    .ok_or_else(|| RPCError::custom_with_error(RPCError::IPC, "Unexpected"))?;
                let search_key = IndexerSearchKey {
                    script: Script {
                        code_hash: TYPE_ID_CODE_HASH,
                        hash_type: ScriptHashType::Type,
                        args: JsonBytes::from_vec(type_id_args.as_bytes().to_vec()),
                    },
                    script_type: IndexerScriptType::Type,
                    ..Default::default()
                };
                let object_vec = data
                    .handle
                    .get_cells(search_key, IndexerOrder::Desc, 1.into(), None)
                    .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?
                    .objects;
                Ok(object_vec
                    .first()
                    .ok_or_else(|| {
                        RPCError::custom(
                            RPCError::IPC,
                            format!("Get type id args failed: {}", type_id_args),
                        )
                    })?
                    .output_data
                    .clone()
                    .ok_or_else(|| RPCError::custom(RPCError::IPC, "Unexpected"))
                    .map(|e| e.into_bytes())?)
            }
            None => Err(RPCError::custom(
                RPCError::IPCIndexerIsDisabled,
                "Query by type id requires enabling Indexer in [rpc.modules]",
            )),
        }
    }

    fn ipc_script_data(&self, script_locator: IpcScriptLocator) -> Result<Bytes> {
        if script_locator.out_point.is_some() {
            return self.ipc_script_data_by_out_point(script_locator);
        }
        if script_locator.type_id_args.is_some() {
            return self.ipc_script_data_by_type_id_args(script_locator);
        }
        return Err(RPCError::custom(
            RPCError::IPC,
            format!("Get out point failed: {:?}", script_locator),
        ));
    }
}

#[async_trait]
impl IpcRpc for IpcRpcImpl {
    fn ipc_call(
        &self,
        script_locator: IpcScriptLocator,
        req: IpcRequest,
        env: Option<IpcEnv>,
    ) -> Result<IpcResponse> {
        let mut dl = Resource::new(self.shared.clone());
        let fmt = req.payload_format;
        let env = self.ipc_env(&mut dl, self.ipc_script_data(script_locator)?, env)?;
        let tx = packed::Transaction::from(env.tx).into_view();
        let mut tx = resolve_transaction(tx, &mut HashSet::new(), &dl, &dl)
            .map_err(|e| RPCError::custom_with_error(RPCError::TransactionFailedToResolve, e))?;
        // Due to the existence of system cell cache, some system scripts will not be obtained from Resource. There is
        // currently no way to disable the system cell cache, so manually override them here.
        for e in tx.resolved_cell_deps.iter_mut() {
            if let Some(CellStatus::Live(meta)) = dl.cell.get(&e.out_point) {
                *e = meta.clone()
            }
        }
        let snapshot = self.shared.cloned_snapshot();
        let pipesctx = PipesCtx::default();
        let script_verifier = TransactionScriptsVerifier::<_, _, Machine>::new_with_generator(
            Arc::new(tx),
            dl,
            snapshot.cloned_consensus(),
            Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header())),
            overload_ckb_syscalls,
            pipesctx.clone(),
        );
        let script_group = script_verifier
            .find_script_group(
                match env.script_group_type {
                    ScriptGroupType::Lock => ckb_script::ScriptGroupType::Lock,
                    ScriptGroupType::Type => ckb_script::ScriptGroupType::Type,
                },
                &env.script_hash.pack(),
            )
            .ok_or_else(|| {
                RPCError::custom_with_error(
                    RPCError::IPC,
                    ckb_script::ScriptError::ScriptNotFound(env.script_hash.pack()),
                )
            })?
            .clone();
        let pipesfin = pipesctx.clone();
        let signal_machine = tokio::sync::watch::channel(ChunkCommand::Resume);
        self.shared.async_handle().spawn({
            async move {
                // Ignore its return value as it is unimportant.
                let _ = script_verifier
                    .create_scheduler(&script_group)
                    .map(|mut scheduler| {
                        // Needs a maximum number of cycles so that the vm can always be stopped.
                        let step = 1024;
                        let step_cycles = snapshot.cloned_consensus().max_block_cycles / step;
                        for _ in 0..step {
                            let result = scheduler.run(RunMode::LimitCycles(step_cycles));
                            if let Err(ckb_vm::Error::CyclesExceeded) = result {
                                if signal_machine.1.has_changed().unwrap_or_default() {
                                    break;
                                }
                                continue;
                            }
                            break;
                        }
                    });
                // If the vm exits unexpectedly and no packet is returned, we will close all pipes.
                // If the vm exits normally, still close it because the script could be not a valid ipc script.
                pipesfin.close();
            }
        });
        // In any case, we will close all pipes after a certain period of time.
        // This ensures that the function always completes if the ipc script is not written as expected.
        let pipesfin = pipesctx.clone();
        let mut signal_timeout = tokio::sync::watch::channel(0);
        self.shared.async_handle().spawn({
            async move {
                let _ = tokio::time::timeout(
                    tokio::time::Duration::from_secs(8),
                    signal_timeout.1.changed(),
                )
                .await;
                pipesfin.close();
            }
        });
        // Defer execution. We have to give ScopeCall a name so that it will be automatically dropped when it leaves
        // the scope.
        let _scope_call = ScopeCall {
            c: || {
                let _ = signal_machine.0.send(ChunkCommand::Stop);
                let _ = signal_timeout.0.send(1);
            },
        };
        let req = RequestPacket::new(
            req.version.value() as u8,
            req.method_id.value(),
            match fmt {
                IpcPayloadFormat::Hex => {
                    let str = req
                        .payload
                        .as_str()
                        .ok_or_else(|| RPCError::custom_with_error(RPCError::IPC, "Payload error"))?
                        .strip_prefix("0x")
                        .ok_or_else(|| {
                            RPCError::custom_with_error(RPCError::IPC, "Payload error")
                        })?;
                    let mut buf = vec![0u8; str.len() / 2];
                    hex_decode(str.as_bytes(), &mut buf)
                        .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?;
                    buf
                }
                IpcPayloadFormat::Json => serde_json::ser::to_vec(&req.payload)
                    .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?,
            },
        );
        pipesctx
            .nw
            .lock()
            .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?
            .write_all(&req.serialize())
            .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?;
        pipesctx
            .nw
            .lock()
            .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?
            .close();
        let resp = ResponsePacket::read_from(
            pipesctx
                .nr
                .lock()
                .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?
                .deref_mut(),
        )
        .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?;
        Ok(IpcResponse {
            version: Uint64::from(resp.version() as u64),
            error_code: Uint64::from(resp.error_code()),
            payload_format: fmt.clone(),
            payload: match fmt {
                IpcPayloadFormat::Hex => Value::String(format!("0x{}", hex_string(resp.payload()))),
                IpcPayloadFormat::Json => {
                    serde_from_str::<Value>(String::from_utf8_lossy(resp.payload()).as_ref())
                        .map_err(|e| RPCError::custom_with_error(RPCError::IPC, e))?
                }
            },
        })
    }
}
