use crate::v2_types::{DataPieceId, TxData};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::snapshot2::Snapshot2Context;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MachineContext<
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
> {
    pub(crate) base_cycles: Arc<Mutex<u64>>,
    pub(crate) snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
}

impl<DL> MachineContext<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub fn new(tx_data: TxData<DL>) -> Self {
        Self {
            base_cycles: Arc::new(Mutex::new(0)),
            snapshot2_context: Arc::new(Mutex::new(Snapshot2Context::new(tx_data))),
        }
    }

    pub fn snapshot2_context(&self) -> &Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>> {
        &self.snapshot2_context
    }

    pub fn set_base_cycles(&mut self, base_cycles: u64) {
        *self.base_cycles.lock().expect("lock") = base_cycles;
    }
}
