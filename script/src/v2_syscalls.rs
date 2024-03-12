use crate::{
    v2_types::{DataPieceId, Message, TxData, VmId},
    ScriptVersion,
};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::snapshot2::Snapshot2Context;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MachineContext<
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
> {
    pub(crate) id: VmId,
    pub(crate) base_cycles: Arc<Mutex<u64>>,
    pub(crate) message_box: Arc<Mutex<Vec<Message>>>,
    pub(crate) snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    pub(crate) script_version: ScriptVersion,
}

impl<DL> MachineContext<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
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

    pub fn set_base_cycles(&mut self, base_cycles: u64) {
        *self.base_cycles.lock().expect("lock") = base_cycles;
    }
}
