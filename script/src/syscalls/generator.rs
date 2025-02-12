use crate::{
    syscalls::{
        Close, CurrentCycles, Debugger, Exec, ExecV2, InheritedFd, LoadBlockExtension, LoadCell,
        LoadCellData, LoadHeader, LoadInput, LoadScript, LoadScriptHash, LoadTx, LoadWitness, Pipe,
        ProcessID, Read, Spawn, VMVersion, Wait, Write,
    },
    types::{CoreMachine, DebugContext, ScriptVersion, SgData, VmContext, VmId},
};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::Syscalls;
use std::sync::Arc;

/// Generate RISC-V syscalls in CKB environment
pub fn generate_ckb_syscalls<DL>(
    vm_id: &VmId,
    sg_data: &Arc<SgData<DL>>,
    vm_context: &VmContext<DL>,
    debug_context: &DebugContext,
) -> Vec<Box<(dyn Syscalls<CoreMachine>)>>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    let mut syscalls: Vec<Box<(dyn Syscalls<CoreMachine>)>> = vec![
        Box::new(LoadScriptHash::new(sg_data)),
        Box::new(LoadTx::new(sg_data)),
        Box::new(LoadCell::new(sg_data)),
        Box::new(LoadInput::new(sg_data)),
        Box::new(LoadHeader::new(sg_data)),
        Box::new(LoadWitness::new(sg_data)),
        Box::new(LoadScript::new(sg_data)),
        Box::new(LoadCellData::new(vm_context)),
        Box::new(Debugger::new(sg_data, debug_context)),
    ];
    let script_version = &sg_data.script_version;
    if script_version >= &ScriptVersion::V1 {
        syscalls.append(&mut vec![
            Box::new(VMVersion::new()),
            Box::new(CurrentCycles::new(vm_context)),
        ]);
    }
    if script_version == &ScriptVersion::V1 {
        syscalls.push(Box::new(Exec::new(sg_data)));
    }
    if script_version >= &ScriptVersion::V2 {
        syscalls.append(&mut vec![
            Box::new(ExecV2::new(vm_id, vm_context)),
            Box::new(LoadBlockExtension::new(sg_data)),
            Box::new(Spawn::new(vm_id, vm_context)),
            Box::new(ProcessID::new(vm_id)),
            Box::new(Pipe::new(vm_id, vm_context)),
            Box::new(Wait::new(vm_id, vm_context)),
            Box::new(Write::new(vm_id, vm_context)),
            Box::new(Read::new(vm_id, vm_context)),
            Box::new(InheritedFd::new(vm_id, vm_context)),
            Box::new(Close::new(vm_id, vm_context)),
        ]);
    }
    #[cfg(test)]
    syscalls.push(Box::new(crate::syscalls::Pause::new(Arc::clone(
        &debug_context.skip_pause,
    ))));
    syscalls
}
