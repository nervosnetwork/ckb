use ckb_jsonrpc_types::{AlertMessage, ChainInfo, PeerState};
use ckb_network_alert::notifier::Notifier as AlertNotifier;
use ckb_shared::shared::Shared;
use ckb_sync::Synchronizer;
use ckb_traits::BlockMedianTimeContext;
use ckb_util::Mutex;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::sync::Arc;

#[rpc(server)]
pub trait StatsRpc {
    #[rpc(name = "get_blockchain_info")]
    fn get_blockchain_info(&self) -> Result<ChainInfo>;

    #[rpc(name = "get_peers_state")]
    fn get_peers_state(&self) -> Result<Vec<PeerState>>;
}

pub(crate) struct StatsRpcImpl {
    pub shared: Shared,
    pub synchronizer: Synchronizer,
    pub alert_notifier: Arc<Mutex<AlertNotifier>>,
}

impl StatsRpc for StatsRpcImpl {
    fn get_blockchain_info(&self) -> Result<ChainInfo> {
        let chain = self.synchronizer.shared.consensus().id.clone();
        let (tip_header, median_time) = {
            let snapshot = self.shared.snapshot();
            let tip_header = snapshot.tip_header().clone();
            let median_time = snapshot.block_median_time(&tip_header.hash());
            (tip_header, median_time)
        };
        let epoch = tip_header.epoch();
        let difficulty = tip_header.difficulty();
        let is_initial_block_download = self
            .synchronizer
            .shared
            .active_chain()
            .is_initial_block_download();
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

    fn get_peers_state(&self) -> Result<Vec<PeerState>> {
        // deprecated
        Ok(self
            .synchronizer
            .shared()
            .state()
            .read_inflight_blocks()
            .blocks_iter()
            .map(|(peer, blocks)| PeerState::new(peer.value(), 0, blocks.len()))
            .collect())
    }
}
