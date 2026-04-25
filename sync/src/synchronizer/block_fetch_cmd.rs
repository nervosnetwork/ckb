use super::block_fetcher::BlockFetcher;
use crate::types::{IBDState, SyncShared};
use ckb_channel::{self as channel, Receiver, select};
use ckb_constant::sync::MAX_TIP_AGE;
use ckb_logger::{debug, info, warn};
use ckb_network::{PeerIndex, ServiceAsyncControl, ServiceControl, SupportProtocols};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::BlockNumber,
    packed::{self, Byte32},
    prelude::*,
};
use std::sync::Arc;
use std::time::Duration;

#[derive(Copy, Clone)]
pub(super) enum CanStart {
    FetchToTarget(BlockNumber),
    Ready,
    MinWorkNotReach,
    AssumeValidNotFound,
}

pub(super) struct FetchCMD {
    pub(super) peers: Vec<PeerIndex>,
    pub(super) ibd_state: IBDState,
}

pub(super) struct BlockFetchCMD {
    pub(super) sync_shared: Arc<SyncShared>,
    pub(super) p2p_control: ServiceControl,
    pub(super) recv: channel::Receiver<FetchCMD>,
    pub(super) can_start: CanStart,
    pub(super) number: BlockNumber,
    pub(super) start_timestamp: u64,
}

impl BlockFetchCMD {
    fn process_fetch_cmd(&mut self, cmd: FetchCMD) {
        let FetchCMD { peers, ibd_state }: FetchCMD = cmd;

        let fetch_blocks_fn = |cmd: &mut BlockFetchCMD, assume_target: BlockNumber| {
            for peer in peers {
                if ckb_stop_handler::has_received_stop_signal() {
                    return;
                }

                let mut fetch_end: BlockNumber = u64::MAX;
                if assume_target != 0 {
                    fetch_end = assume_target
                }

                if let Some(fetch) =
                    BlockFetcher::new(Arc::clone(&cmd.sync_shared), peer, ibd_state)
                        .fetch(fetch_end)
                {
                    for item in fetch {
                        if ckb_stop_handler::has_received_stop_signal() {
                            return;
                        }
                        let ctrl = cmd.p2p_control.clone();
                        let handle = cmd.sync_shared.shared().async_handle();
                        handle.spawn(BlockFetchCMD::send_getblocks(item, ctrl, peer));
                    }
                }
            }
        };

        match self.can_start() {
            CanStart::FetchToTarget(assume_target) => fetch_blocks_fn(self, assume_target),
            CanStart::Ready => fetch_blocks_fn(self, BlockNumber::MAX),
            CanStart::MinWorkNotReach => {
                let best_known = self.sync_shared.state().shared_best_header_ref();
                let number = best_known.number();
                if number != self.number && (number - self.number).is_multiple_of(10000) {
                    self.number = number;
                    info!(
                        "The current best known header number: {}, total difficulty: {:#x}. \
                                 Block download minimum requirements: header number: 500_000, total difficulty: {:#x}.",
                        number,
                        best_known.total_difficulty(),
                        self.sync_shared.state().min_chain_work()
                    );
                }
            }
            CanStart::AssumeValidNotFound => {
                let state = self.sync_shared.state();
                let shared = self.sync_shared.shared();
                let best_known = state.shared_best_header_ref();
                let number = best_known.number();
                let assume_valid_target: Byte32 = shared
                    .assume_valid_targets()
                    .as_ref()
                    .and_then(|targets| targets.first())
                    .map(Pack::pack)
                    .expect("assume valid target must exist");

                if number != self.number && (number - self.number).is_multiple_of(10000) {
                    self.number = number;
                    let remaining_headers_sync_log = self.reaming_headers_sync_log();

                    info!(
                        "best known header {}-{}, \
                                 CKB is syncing to latest Header to find the assume valid target: {}. \
                                 Please wait. {}",
                        number,
                        best_known.hash(),
                        assume_valid_target,
                        remaining_headers_sync_log
                    );
                }
            }
        }
    }

    fn reaming_headers_sync_log(&self) -> String {
        match self.calc_time_need_to_reach_latest_tip_header() {
            Some(remaining) => {
                let secs = remaining.as_secs();
                match secs {
                    0 => "Almost synced.".to_string(),
                    1..=59 => format!("Need {} seconds to sync to the latest Header.", secs),
                    60..=3599 => {
                        format!("Need {} minutes to sync to the latest Header.", secs / 60)
                    }
                    _ => {
                        let hours = secs / 3600;
                        let minutes = (secs % 3600) / 60;
                        format!(
                            "Need {} hours {} minutes to sync to the latest Header.",
                            hours, minutes
                        )
                    }
                }
            }
            None => "".to_string(),
        }
    }

    // Timeline:
    //
    // |-------------------|--------------------------------|------------|---->
    // Genesis  (shared best timestamp)                     |           now
    // |                   |                                |            |
    // |             (Sync point)                  (CKB process start)   |
    // |                   |                                             |
    // |--Synced Part------|------------ Remain to Sync -----------------|
    // |                                                                 |
    // |------------------- CKB Chain Age -------------------------------|
    //
    fn calc_time_need_to_reach_latest_tip_header(&self) -> Option<Duration> {
        let genesis_timestamp = self
            .sync_shared
            .consensus()
            .genesis_block()
            .header()
            .timestamp();
        let shared_best_timestamp = self.sync_shared.state().shared_best_header().timestamp();

        let ckb_process_start_timestamp = self.start_timestamp;

        let now_timestamp = unix_time_as_millis();

        // Use floating point to avoid integer division precision loss
        let ckb_chain_age = now_timestamp.checked_sub(genesis_timestamp)? as f64;
        let ckb_process_age = now_timestamp.checked_sub(ckb_process_start_timestamp)? as f64;
        let has_synced_headers_age = shared_best_timestamp.checked_sub(genesis_timestamp)? as f64;

        if ckb_process_age <= 0.0 || has_synced_headers_age <= 0.0 {
            return None;
        }

        let ckb_sync_header_speed = has_synced_headers_age / ckb_process_age;
        let sync_all_headers_timecost = ckb_chain_age / ckb_sync_header_speed;
        let sync_remaining_headers_needed = sync_all_headers_timecost - ckb_process_age;

        if sync_remaining_headers_needed <= 0.0 {
            Some(Duration::from_millis(0))
        } else {
            Some(Duration::from_millis(sync_remaining_headers_needed as u64))
        }
    }

    pub(super) fn run(&mut self, stop_signal: Receiver<()>) {
        loop {
            select! {
                recv(self.recv) -> msg => {
                    if let Ok(cmd) = msg {
                        self.process_fetch_cmd(cmd)
                    }
                }
                recv(stop_signal) -> _ => {
                    info!("BlockDownload received exit signal, exit now");
                    return;
                }
            }
        }
    }

    fn can_start(&mut self) -> CanStart {
        if let CanStart::Ready = self.can_start {
            return self.can_start;
        }

        let shared = self.sync_shared.shared();
        let state = self.sync_shared.state();

        let min_work_reach = |flag: &mut CanStart| {
            if state.min_chain_work_ready() {
                *flag = CanStart::AssumeValidNotFound;
            }
        };

        let assume_valid_target_find = |flag: &mut CanStart| {
            let mut assume_valid_targets = shared.assume_valid_targets();
            if let Some(ref targets) = *assume_valid_targets {
                if targets.is_empty() {
                    assume_valid_targets.take();
                    *flag = CanStart::Ready;
                    return;
                }
                let first_target = targets
                    .first()
                    .expect("has checked targets is not empty, assume valid target must exist");
                match shared.header_map().get(&first_target.into()) {
                    Some(header) => {
                        if matches!(*flag, CanStart::FetchToTarget(fetch_target) if fetch_target == header.number())
                        {
                            // BlockFetchCMD has set the fetch target, no need to set it again
                        } else {
                            *flag = CanStart::FetchToTarget(header.number());
                            info!(
                                "assume valid target found in header_map; CKB will start fetch blocks to {:?} now",
                                header.number_and_hash()
                            );
                        }
                        // Blocks that are no longer in the scope of ibd must be forced to verify
                        if unix_time_as_millis().saturating_sub(header.timestamp()) < MAX_TIP_AGE {
                            assume_valid_targets.take();
                            warn!(
                                "the duration gap between 'assume valid target' and 'now' is less than 24h; CKB will ignore the specified assume valid target and do full verification from now on"
                            );
                        }
                    }
                    None => {
                        // Best known already not in the scope of ibd, it means target is invalid
                        if unix_time_as_millis()
                            .saturating_sub(state.shared_best_header_ref().timestamp())
                            < MAX_TIP_AGE
                        {
                            warn!(
                                "the duration gap between 'shared_best_header' and 'now' is less than 24h, but CKB haven't found the assume valid target in header_map; CKB will ignore the specified assume valid target and do full verification from now on"
                            );
                            *flag = CanStart::Ready;
                            assume_valid_targets.take();
                        }
                    }
                }
            } else {
                *flag = CanStart::Ready;
            }
        };

        match self.can_start {
            CanStart::FetchToTarget(_) => {
                assume_valid_target_find(&mut self.can_start);
                self.can_start
            }
            CanStart::Ready => self.can_start,
            CanStart::MinWorkNotReach => {
                min_work_reach(&mut self.can_start);
                if let CanStart::AssumeValidNotFound = self.can_start {
                    assume_valid_target_find(&mut self.can_start);
                }
                self.can_start
            }
            CanStart::AssumeValidNotFound => {
                assume_valid_target_find(&mut self.can_start);
                self.can_start
            }
        }
    }

    async fn send_getblocks(v_fetch: Vec<packed::Byte32>, nc: ServiceControl, peer: PeerIndex) {
        let content = packed::GetBlocks::new_builder()
            .block_hashes(v_fetch.clone())
            .build();
        let message = packed::SyncMessage::new_builder().set(content).build();

        debug!("send_getblocks len={:?} to peer={}", v_fetch.len(), peer);
        if let Err(err) = Into::<ServiceAsyncControl>::into(nc)
            .send_message_to(
                peer,
                SupportProtocols::Sync.protocol_id(),
                message.as_bytes(),
            )
            .await
        {
            debug!("synchronizer sending GetBlocks error: {:?}", err);
        }
    }
}
