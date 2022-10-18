use ckb_jsonrpc_types::{AlertMessage, ChainInfo, DeploymentInfo, DeploymentPos, DeploymentsInfo};
use ckb_network_alert::notifier::Notifier as AlertNotifier;
use ckb_shared::shared::Shared;
use ckb_traits::HeaderProvider;
use ckb_types::prelude::Unpack;
use ckb_util::Mutex;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::BTreeMap;
use std::sync::Arc;

/// RPC Module Stats for getting various statistic data.
#[rpc(server)]
pub trait StatsRpc {
    /// Returns statistics about the chain.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_blockchain_info",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "alerts": [
    ///       {
    ///         "id": "0x2a",
    ///         "message": "An example alert message!",
    ///         "notice_until": "0x24bcca57c00",
    ///         "priority": "0x1"
    ///       }
    ///     ],
    ///     "chain": "ckb",
    ///     "difficulty": "0x1f4003",
    ///     "epoch": "0x7080018000001",
    ///     "is_initial_block_download": true,
    ///     "median_time": "0x5cd2b105"
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_blockchain_info")]
    fn get_blockchain_info(&self) -> Result<ChainInfo>;

    /// Returns statistics about the chain.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_deployments_info",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "epoch": "0x1",
    ///     "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///        "deployments": {
    ///            "Testdummy": {
    ///                "bit": 1,
    ///                "min_activation_epoch": "0x0",
    ///                "start": "0x0",
    ///                "state": "Failed",
    ///                "timeout": "0x0"
    ///            }
    ///        }
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_deployments_info")]
    fn get_deployments_info(&self) -> Result<DeploymentsInfo>;
}

pub(crate) struct StatsRpcImpl {
    pub shared: Shared,
    pub alert_notifier: Arc<Mutex<AlertNotifier>>,
}

impl StatsRpc for StatsRpcImpl {
    fn get_blockchain_info(&self) -> Result<ChainInfo> {
        let chain = self.shared.consensus().id.clone();
        let (tip_header, median_time) = {
            let snapshot = self.shared.snapshot();
            let tip_header = snapshot.tip_header().clone();
            let median_time = snapshot.block_median_time(
                &tip_header.hash(),
                self.shared.consensus().median_time_block_count(),
            );
            (tip_header, median_time)
        };
        let epoch = if tip_header.is_genesis() {
            self.shared
                .consensus()
                .genesis_epoch_ext()
                .number_with_fraction(0)
        } else {
            tip_header.epoch()
        };
        let difficulty = tip_header.difficulty();
        let is_initial_block_download = self.shared.is_initial_block_download();
        let alerts: Vec<AlertMessage> = {
            let now = faketime::unix_time_as_millis();
            let mut notifier = self.alert_notifier.lock();
            notifier.clear_expired_alerts(now);
            notifier
                .noticed_alerts()
                .into_iter()
                .map(Into::into)
                .collect()
        };

        Ok(ChainInfo {
            chain,
            median_time: median_time.into(),
            epoch: epoch.into(),
            difficulty,
            is_initial_block_download,
            alerts,
        })
    }

    fn get_deployments_info(&self) -> Result<DeploymentsInfo> {
        let snapshot = self.shared.snapshot();
        let deployments: BTreeMap<DeploymentPos, DeploymentInfo> = self
            .shared
            .consensus()
            .deployments
            .clone()
            .into_iter()
            .filter_map(|(pos, deployment)| {
                self.shared
                    .consensus()
                    .versionbits_state(pos, snapshot.tip_header(), snapshot.as_ref())
                    .map(|state| {
                        let mut info: DeploymentInfo = deployment.into();
                        info.state = state.into();
                        (pos.into(), info)
                    })
            })
            .collect();

        Ok(DeploymentsInfo {
            hash: snapshot.tip_hash().unpack(),
            epoch: snapshot.tip_header().epoch().number().into(),
            deployments,
        })
    }
}
