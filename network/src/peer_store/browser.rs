use idb::{
    DatabaseEvent, Factory, IndexParams, KeyPath, ObjectStoreParams, TransactionMode,
    TransactionResult,
};
use p2p::runtime;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc::channel, OnceCell};

use std::path::Path;

use crate::errors::PeerStoreError;

static DB: OnceCell<Storage> = OnceCell::const_new();

#[derive(Deserialize, Serialize, Debug)]
pub struct KV {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

struct Request {
    cmd: CommandRequest,
    resp: tokio::sync::oneshot::Sender<CommandResponse>,
}

enum CommandResponse {
    Read { value: Option<Vec<u8>> },
    Put,
    Shutdown,
}

enum CommandRequest {
    Read { key: Vec<u8> },
    Put { kv: KV },
    Shutdown,
}

impl std::fmt::Debug for CommandResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandResponse::Read { .. } => write!(f, "Read"),
            CommandResponse::Put { .. } => write!(f, "Put"),
            CommandResponse::Shutdown => write!(f, "Shutdown"),
        }
    }
}

impl std::fmt::Debug for CommandRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandRequest::Read { .. } => write!(f, "Read"),
            CommandRequest::Put { .. } => write!(f, "Put"),
            CommandRequest::Shutdown => write!(f, "Shutdown"),
        }
    }
}

pub async fn get_db<P: AsRef<Path>>(path: P) -> &'static Storage {
    DB.get_or_init(|| Storage::new(path)).await
}

const STORE_NAME: &str = "main-store";

#[derive(Clone)]
pub struct Storage {
    chan: tokio::sync::mpsc::Sender<Request>,
}

impl Storage {
    pub async fn new<P: AsRef<Path>>(path: P) -> Self {
        let factory = Factory::new().unwrap();
        let database_name = path.as_ref().to_str().unwrap().to_owned();
        let mut open_request = factory.open(&database_name, Some(1)).unwrap();
        open_request.on_upgrade_needed(move |event| {
            let database = event.database().unwrap();
            let store_params = ObjectStoreParams::new();

            let store = database
                .create_object_store(STORE_NAME, store_params)
                .unwrap();
            let mut index_params = IndexParams::new();
            index_params.unique(true);
            store
                .create_index("key", KeyPath::new_single("key"), Some(index_params))
                .unwrap();
        });
        let db = open_request.await.unwrap();
        let (tx, mut rx) = channel(128);

        runtime::spawn(async move {
            loop {
                let request: Request = rx.recv().await.unwrap();
                match request.cmd {
                    CommandRequest::Read { key } => {
                        let tran = db
                            .transaction(&[STORE_NAME], TransactionMode::ReadOnly)
                            .unwrap();
                        let store = tran.object_store(STORE_NAME).unwrap();
                        let key = serde_wasm_bindgen::to_value(&key).unwrap();
                        let value = store
                            .get(key)
                            .unwrap()
                            .await
                            .unwrap()
                            .map(|v| serde_wasm_bindgen::from_value::<KV>(v).unwrap().value);
                        assert_eq!(TransactionResult::Committed, tran.await.unwrap());
                        request.resp.send(CommandResponse::Read { value }).unwrap()
                    }
                    CommandRequest::Put { kv } => {
                        let tran = db
                            .transaction(&[STORE_NAME], TransactionMode::ReadWrite)
                            .unwrap();
                        let store = tran.object_store(STORE_NAME).unwrap();

                        let key = serde_wasm_bindgen::to_value(&kv.key).unwrap();
                        let value = serde_wasm_bindgen::to_value(&kv).unwrap();
                        store.put(&value, Some(&key)).unwrap().await.unwrap();
                        assert_eq!(
                            TransactionResult::Committed,
                            tran.commit().unwrap().await.unwrap()
                        );
                        request.resp.send(CommandResponse::Put).unwrap();
                    }
                    CommandRequest::Shutdown => {
                        request.resp.send(CommandResponse::Shutdown).unwrap();
                        break;
                    }
                }
            }
        });

        Self { chan: tx }
    }

    pub async fn get<K: AsRef<[u8]>>(&self, key: K) -> Result<Option<Vec<u8>>, PeerStoreError> {
        let value = send_command(
            &self.chan,
            CommandRequest::Read {
                key: key.as_ref().to_vec(),
            },
        )
        .await;
        if let CommandResponse::Read { value } = value {
            return Ok(value);
        } else {
            unreachable!()
        }
    }

    pub async fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), PeerStoreError> {
        let kv = KV { key, value };

        send_command(&self.chan, CommandRequest::Put { kv }).await;
        Ok(())
    }

    pub async fn shutdown(&self) {
        if let CommandResponse::Shutdown = send_command(&self.chan, CommandRequest::Shutdown).await
        {
        } else {
            unreachable!()
        }
    }
}

async fn send_command(
    chan: &tokio::sync::mpsc::Sender<Request>,
    cmd: CommandRequest,
) -> CommandResponse {
    let (tx, rx) = tokio::sync::oneshot::channel();
    chan.send(Request { cmd, resp: tx }).await.unwrap();
    rx.await.unwrap()
}
