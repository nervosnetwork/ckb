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

/// RPC Module Alert for network alerts.
///
/// An alert is a message about critical problems to be broadcast to all nodes via the p2p network.
///
/// The alerts must be signed by 2-of-4 signatures, where the public keys are hard-coded in the source code
/// and belong to early CKB developers.
#[rpc(server)]
pub trait AlertRpc {
    /// Sends an alert.
    ///
    /// This RPC returns `null` on success.
    ///
    /// ## Errors
    ///
    /// * [`AlertFailedToVerifySignatures (-1000)`](../enum.RPCError.html#variant.AlertFailedToVerifySignatures) - Some signatures in the request are invalid.
    /// * [`P2PFailedToBroadcast (-101)`](../enum.RPCError.html#variant.P2PFailedToBroadcast) - Alert is saved locally but has failed to broadcast to the P2P network.
    /// * `InvalidParams (-32602)` - The time specified in `alert.notice_until` must be in the future.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "jsonrpc": "2.0",
    ///   "method": "send_alert",
    ///   "params": [
    ///     {
    ///       "id": "0x1",
    ///       "cancel": "0x0",
    ///       "priority": "0x1",
    ///       "message": "An example alert message!",
    ///       "notice_until": "0x24bcca57c00",
    ///       "signatures": [
    ///         "0xbd07059aa9a3d057da294c2c4d96fa1e67eeb089837c87b523f124239e18e9fc7d11bb95b720478f7f937d073517d0e4eb9a91d12da5c88a05f750362f4c214dd0",
    ///         "0x0242ef40bb64fe3189284de91f981b17f4d740c5e24a3fc9b70059db6aa1d198a2e76da4f84ab37549880d116860976e0cf81cd039563c452412076ebffa2e4453"
    ///       ]
    ///     }
    ///   ],
    ///   "id": 42
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "error": {
    ///     "code": -1000,
    ///     "data": "SigNotEnough",
    ///     "message":"AlertFailedToVerifySignatures: The count of sigs less than threshold."
    ///   },
    ///   "jsonrpc": "2.0",
    ///   "result": null,
    ///   "id": 42
    /// }
    /// ```
    #[rpc(name = "send_alert")]
    fn send_alert(&self, alert: Alert) -> Result<()>;
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
