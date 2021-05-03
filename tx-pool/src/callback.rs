use super::component::TxEntry;
use crate::error::Reject;
use crate::pool::TxPool;
use ckb_async_runtime::Handle;
use ckb_jsonrpc_types::TransactionView;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf, time};

/// Callback boxed fn pointer wrapper
pub type Callback = Box<dyn Fn(&mut TxPool, &TxEntry) + Sync + Send>;
/// Proposed Callback boxed fn pointer wrapper
pub type ProposedCallback = Box<dyn Fn(&mut TxPool, &TxEntry, bool) + Sync + Send>;
/// Reject Callback boxed fn pointer wrapper
pub type RejectCallback = Box<dyn Fn(&mut TxPool, &TxEntry, Reject) + Sync + Send>;

/// Struct hold callbacks
pub struct Callbacks {
    pub(crate) pending: Option<Callback>,
    pub(crate) proposed: Option<ProposedCallback>,
    pub(crate) committed: Option<Callback>,
    pub(crate) reject: Option<RejectCallback>,
    path: PathBuf,
    handle: Handle,
}

impl Callbacks {
    /// Construct new Callbacks
    pub fn new(handle: Handle, path: PathBuf) -> Self {
        Callbacks {
            pending: None,
            proposed: None,
            committed: None,
            reject: None,
            path,
            handle,
        }
    }

    /// Register a new pending callback
    pub fn register_pending(&mut self, callback: Callback) {
        self.pending = Some(callback);
    }

    /// Register a new proposed callback
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.proposed = Some(callback);
    }

    /// Register a new committed callback
    pub fn register_committed(&mut self, callback: Callback) {
        self.committed = Some(callback);
    }

    /// Register a new abandon callback
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.reject = Some(callback);
    }

    /// Call on after pending
    pub fn call_pending(&self, tx_pool: &mut TxPool, entry: &TxEntry) {
        if let Some(call) = &self.pending {
            call(tx_pool, entry)
        }
    }

    /// Call on after proposed
    pub fn call_proposed(&self, tx_pool: &mut TxPool, entry: &TxEntry, new: bool) {
        if let Some(call) = &self.proposed {
            call(tx_pool, entry, new)
        }
    }

    /// Call on after proposed
    pub fn call_committed(&self, tx_pool: &mut TxPool, entry: &TxEntry) {
        if let Some(call) = &self.committed {
            call(tx_pool, entry)
        }
    }

    /// Call on after reject
    pub fn call_reject(&self, tx_pool: &mut TxPool, entry: &TxEntry, reject: Reject) {
        let path = self.path.join("reject");
        let tx = RejectDump {
            tx: entry.transaction().to_owned().into(),
            reason: reject.to_string(),
        };
        self.handle.spawn_blocking(|| dump_reject(path, tx));
        if let Some(call) = &self.reject {
            call(tx_pool, entry, reject)
        }
    }
}

#[derive(Serialize, Deserialize)]
struct RejectDump {
    tx: TransactionView,
    reason: String,
}

/// Dump reject tx to `{root}/data/tx_pool/reject/{tx_hash}` and keep only the last 20 files
fn dump_reject(path: PathBuf, tx: RejectDump) {
    let name = path.join(format!("{:x}", tx.tx.hash));
    let reject_tx = serde_json::to_string(&tx).unwrap();
    let _ignore = fs::write(name, reject_tx);
    clean(path)
}

fn clean(path: PathBuf) {
    if let Ok(Ok(list)) =
        fs::read_dir(path).map(|a| a.collect::<Result<Vec<fs::DirEntry>, io::Error>>())
    {
        if list.len() > 20 {
            let mut time_list = list
                .into_iter()
                .map(|dir| (dir.path(), dir.metadata().and_then(|a| a.created()).ok()))
                .collect::<Vec<(PathBuf, Option<time::SystemTime>)>>();
            // None always less than Some
            time_list.sort_by(|a, b| a.1.cmp(&b.1));
            let index = time_list.len() - 20;
            for (path, _) in time_list.iter().take(index) {
                let _ignore = fs::remove_file(path);
            }
        }
    }
}
