use crate::error::RPCError;
use ckb_jsonrpc_types::Alert;
use ckb_logger::error;
use ckb_network::{bytes, NetworkController};
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_sync::NetworkProtocol;
use ckb_types::{packed, prelude::*};
use ckb_util::Mutex;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::sync::Arc;

#[rpc(server)]
pub trait AlertRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_alert","params": [{}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "send_alert")]
    fn send_alert(&self, _alert: Alert) -> Result<()>;
}

pub(crate) struct AlertRpcImpl {
    network_controller: NetworkController,
    verifier: Arc<AlertVerifier>,
    notifier: Arc<Mutex<AlertNotifier>>,
}

impl AlertRpcImpl {
    pub fn new(
        verifier: Arc<AlertVerifier>,
        notifier: Arc<Mutex<AlertNotifier>>,
        network_controller: NetworkController,
    ) -> Self {
        AlertRpcImpl {
            network_controller,
            verifier,
            notifier,
        }
    }
}

impl AlertRpc for AlertRpcImpl {
    fn send_alert(&self, alert: Alert) -> Result<()> {
        let alert: packed::Alert = alert.into();
        let now_ms = faketime::unix_time_as_millis();
        let notice_until: u64 = alert.raw().notice_until().unpack();
        if notice_until < now_ms {
            return Err(RPCError::custom(
                RPCError::Invalid,
                format!(
                    "expired alert, notice_until: {} server: {}",
                    notice_until, now_ms
                ),
            ));
        }

        let result = self.verifier.verify_signatures(&alert);

        match result {
            Ok(()) => {
                if let Err(err) = self.network_controller.broadcast(
                    NetworkProtocol::ALERT.into(),
                    bytes::Bytes::from(alert.as_slice().to_vec()),
                ) {
                    error!("Broadcast alert failed: {:?}", err);
                }
                // set self node notifier
                self.notifier.lock().add(&alert);
                Ok(())
            }
            Err(e) => Err(RPCError::custom(RPCError::Invalid, format!("{:#}", e))),
        }
    }
}
