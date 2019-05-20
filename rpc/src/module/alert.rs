use crate::error::RPCError;
use ckb_alert_system::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_core::alert::Alert as CoreAlert;
use ckb_network::NetworkController;
use ckb_protocol::AlertMessage;
use ckb_sync::NetworkProtocol;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use jsonrpc_types::Alert;
use std::sync::Arc;

#[rpc]
pub trait AlertRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_alert","params": [{}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "send_alert")]
    fn send_alert(&self, _alert: Alert) -> Result<()>;
}

pub(crate) struct AlertRpcImpl {
    network_controller: NetworkController,
    verifier: Arc<AlertVerifier>,
    notifier: Arc<AlertNotifier>,
}

impl AlertRpcImpl {
    pub fn new(
        verifier: Arc<AlertVerifier>,
        notifier: Arc<AlertNotifier>,
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
        let alert: CoreAlert = alert.into();

        let result = self.verifier.verify_signatures(&alert);

        match result {
            Ok(()) => {
                let fbb = &mut FlatBufferBuilder::new();
                let message = AlertMessage::build_alert(fbb, &alert);
                fbb.finish(message, None);
                let data = fbb.finished_data().into();
                if let Err(err) = self
                    .network_controller
                    .broadcast(NetworkProtocol::ALERT.into(), data)
                {
                    log::error!(target: "rpc", "Broadcast alert failed: {:?}", err);
                }
                // set self node notifier
                self.notifier.add(Arc::new(alert));
                Ok(())
            }
            Err(e) => Err(RPCError::custom(RPCError::Invalid, e.to_string())),
        }
    }
}
