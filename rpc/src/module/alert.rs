use crate::error::RPCError;
use ckb_jsonrpc_types::Alert;
use ckb_logger::error;
use ckb_network::{NetworkController, SupportProtocols};
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
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
            return Err(RPCError::invalid_params(format!(
                "Expected `params[0].notice_until` in the future (> {}), got {}",
                now_ms, notice_until
            )));
        }

        let result = self.verifier.verify_signatures(&alert);

        match result {
            Ok(()) => {
                // set self node notifier
                self.notifier.lock().add(&alert);

                self.network_controller
                    .broadcast(SupportProtocols::Alert.protocol_id(), alert.as_bytes())
                    .map_err(|err| {
                        error!("Broadcast alert failed: {:?}", err);
                        RPCError::custom_with_error(RPCError::P2PFailedToBroadcast, err)
                    })
            }
            Err(e) => Err(RPCError::custom_with_error(
                RPCError::AlertFailedToVerifySignatures,
                e,
            )),
        }
    }
}
