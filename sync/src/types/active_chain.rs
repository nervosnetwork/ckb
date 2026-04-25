use super::sync_shared::{SyncShared, SyncState};
use crate::utils::async_send_message;
use ckb_constant::sync::MAX_HEADERS_LEN;
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex, SupportProtocols};
use ckb_shared::{
    Snapshot,
    block_status::BlockStatus,
    shared::Shared,
    types::{HeaderIndex, HeaderIndexView},
};
use ckb_store::{ChainDB, ChainStore};
use ckb_types::BlockNumberAndHash;
use ckb_types::{
    U256,
    core::{self, BlockNumber},
    packed::{self, Byte32},
    prelude::*,
};
use std::cmp;
use std::sync::Arc;
use std::time::{Duration, Instant};

// TODO: Need discussed
const GET_HEADERS_TIMEOUT: Duration = Duration::from_secs(15);
// 2 ** 13 < 6 * 1800 < 2 ** 14
const ONE_DAY_BLOCK_NUMBER: u64 = 8192;

/** ActiveChain captures a point-in-time view of indexed chain of blocks. */
#[derive(Clone)]
pub struct ActiveChain {
    sync_shared: SyncShared,
    snapshot: Arc<Snapshot>,
}

impl ActiveChain {
    pub(super) fn new(sync_shared: SyncShared, snapshot: Arc<Snapshot>) -> Self {
        ActiveChain {
            sync_shared,
            snapshot,
        }
    }
}

#[doc(hidden)]
impl ActiveChain {
    pub(crate) fn sync_shared(&self) -> &SyncShared {
        &self.sync_shared
    }

    pub fn shared(&self) -> &Shared {
        self.sync_shared.shared()
    }

    fn store(&self) -> &ChainDB {
        self.sync_shared.store()
    }

    pub fn state(&self) -> &SyncState {
        self.sync_shared.state()
    }

    fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn get_block_hash(&self, number: BlockNumber) -> Option<packed::Byte32> {
        self.snapshot().get_block_hash(number)
    }

    pub fn get_block(&self, h: &packed::Byte32) -> Option<core::BlockView> {
        self.store().get_block(h)
    }

    pub fn get_block_header(&self, h: &packed::Byte32) -> Option<core::HeaderView> {
        self.store().get_block_header(h)
    }

    pub fn get_block_ext(&self, h: &packed::Byte32) -> Option<core::BlockExt> {
        self.snapshot().get_block_ext(h)
    }

    pub fn get_block_filter(&self, hash: &packed::Byte32) -> Option<packed::Bytes> {
        self.store().get_block_filter(hash)
    }

    pub fn get_block_filter_hash(&self, hash: &packed::Byte32) -> Option<packed::Byte32> {
        self.store().get_block_filter_hash(hash)
    }

    pub fn get_latest_built_filter_block_number(&self) -> BlockNumber {
        self.snapshot
            .get_latest_built_filter_data_block_hash()
            .and_then(|hash| self.snapshot.get_block_number(&hash))
            .unwrap_or_default()
    }

    pub fn total_difficulty(&self) -> &U256 {
        self.snapshot.total_difficulty()
    }

    pub fn tip_header(&self) -> core::HeaderView {
        self.snapshot.tip_header().clone()
    }

    pub fn tip_hash(&self) -> Byte32 {
        self.snapshot.tip_hash()
    }

    pub fn tip_number(&self) -> BlockNumber {
        self.snapshot.tip_number()
    }

    pub fn epoch_ext(&self) -> core::EpochExt {
        self.snapshot.epoch_ext().clone()
    }

    pub fn is_main_chain(&self, hash: &packed::Byte32) -> bool {
        self.snapshot.is_main_chain(hash)
    }
    pub fn is_unverified_chain(&self, hash: &packed::Byte32) -> bool {
        self.store().get_block_epoch_index(hash).is_some()
    }

    pub fn is_initial_block_download(&self) -> bool {
        self.shared().is_initial_block_download()
    }
    pub fn unverified_tip_header(&self) -> HeaderIndex {
        self.shared().get_unverified_tip()
    }

    pub fn unverified_tip_hash(&self) -> Byte32 {
        self.unverified_tip_header().hash()
    }

    pub fn unverified_tip_number(&self) -> BlockNumber {
        self.unverified_tip_header().number()
    }

    pub fn get_ancestor(&self, base: &Byte32, number: BlockNumber) -> Option<HeaderIndexView> {
        self.get_ancestor_internal(base, number, false)
    }

    pub fn get_ancestor_with_unverified(
        &self,
        base: &Byte32,
        number: BlockNumber,
    ) -> Option<HeaderIndexView> {
        self.get_ancestor_internal(base, number, true)
    }

    fn get_ancestor_internal(
        &self,
        base: &Byte32,
        number: BlockNumber,
        with_unverified: bool,
    ) -> Option<HeaderIndexView> {
        let tip_number = {
            if with_unverified {
                self.unverified_tip_number()
            } else {
                self.tip_number()
            }
        };

        let block_is_on_chain_fn = |hash: &Byte32| {
            if with_unverified {
                self.is_unverified_chain(hash)
            } else {
                self.is_main_chain(hash)
            }
        };

        let get_header_view_fn = |hash: &Byte32, store_first: bool| {
            self.sync_shared.get_header_index_view(hash, store_first)
        };

        let fast_scanner_fn = |number: BlockNumber, current: BlockNumberAndHash| {
            // shortcut to return an ancestor block
            if current.number <= tip_number && block_is_on_chain_fn(&current.hash) {
                self.get_block_hash(number)
                    .and_then(|hash| self.sync_shared.get_header_index_view(&hash, true))
            } else {
                None
            }
        };

        self.sync_shared
            .get_header_index_view(base, false)?
            .get_ancestor(tip_number, number, get_header_view_fn, fast_scanner_fn)
    }

    pub fn get_locator(&self, start: BlockNumberAndHash) -> Vec<Byte32> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let mut index = start.number();
        let mut base = start.hash();

        loop {
            let header_hash = self
                .get_ancestor(&base, index)
                .unwrap_or_else(|| {
                    panic!(
                        "index calculated in get_locator: \
                         start: {:?}, base: {}, step: {}, locators({}): {:?}.",
                        start,
                        base,
                        step,
                        locator.len(),
                        locator,
                    )
                })
                .hash();
            locator.push(header_hash.clone());

            if locator.len() >= 10 {
                step <<= 1;
            }

            if index < step * 2 {
                // Insert some low-height blocks in the locator
                // to quickly start parallel ibd block downloads
                // and it should not be too much
                //
                // 100 * 365 * 86400 / 8 = 394200000  100 years block number
                // 2 ** 29 = 536870912
                // 2 ** 13 = 8192
                // 52 = 10 + 29 + 13
                if locator.len() < 52 && index > ONE_DAY_BLOCK_NUMBER {
                    index >>= 1;
                    base = header_hash;
                    continue;
                }
                // always include genesis hash
                if index != 0 {
                    locator.push(self.sync_shared.consensus().genesis_hash());
                }
                break;
            }
            index -= step;
            base = header_hash;
        }
        locator
    }

    pub fn last_common_ancestor(
        &self,
        pa: &BlockNumberAndHash,
        pb: &BlockNumberAndHash,
    ) -> Option<BlockNumberAndHash> {
        let (mut m_left, mut m_right) = if pa.number() > pb.number() {
            (pb.clone(), pa.clone())
        } else {
            (pa.clone(), pb.clone())
        };

        m_right = self
            .get_ancestor(&m_right.hash(), m_left.number())?
            .number_and_hash();
        if m_left == m_right {
            return Some(m_left);
        }
        debug_assert!(m_left.number() == m_right.number());

        while m_left != m_right {
            m_left = self
                .get_ancestor(&m_left.hash(), m_left.number() - 1)?
                .number_and_hash();
            m_right = self
                .get_ancestor(&m_right.hash(), m_right.number() - 1)?
                .number_and_hash();
        }
        Some(m_left)
    }

    pub fn locate_latest_common_block(
        &self,
        _hash_stop: &Byte32,
        locator: &[Byte32],
    ) -> Option<BlockNumber> {
        if locator.is_empty() {
            return None;
        }

        let locator_hash = locator.last().expect("empty checked");
        if locator_hash != &self.sync_shared.consensus().genesis_hash() {
            return None;
        }

        // iterator are lazy
        let (index, latest_common) = locator
            .iter()
            .enumerate()
            .map(|(index, hash)| (index, self.snapshot.get_block_number(hash)))
            .find(|(_index, number)| number.is_some())
            .expect("locator last checked");

        if index == 0 || latest_common == Some(0) {
            return latest_common;
        }

        if let Some(header) = locator
            .get(index - 1)
            .and_then(|hash| self.sync_shared.store().get_block_header(hash))
        {
            let mut block_hash = header.data().raw().parent_hash();
            loop {
                let block_header = match self.sync_shared.store().get_block_header(&block_hash) {
                    None => break latest_common,
                    Some(block_header) => block_header,
                };

                if let Some(block_number) = self.snapshot.get_block_number(&block_hash) {
                    return Some(block_number);
                }

                block_hash = block_header.data().raw().parent_hash();
            }
        } else {
            latest_common
        }
    }

    pub fn get_locator_response(
        &self,
        block_number: BlockNumber,
        hash_stop: &Byte32,
    ) -> Vec<core::HeaderView> {
        let tip_number = self.tip_header().number();
        let max_height = cmp::min(
            block_number + 1 + MAX_HEADERS_LEN as BlockNumber,
            tip_number + 1,
        );
        (block_number + 1..max_height)
            .filter_map(|block_number| self.snapshot.get_block_hash(block_number))
            .take_while(|block_hash| block_hash != hash_stop)
            .filter_map(|block_hash| self.sync_shared.store().get_block_header(&block_hash))
            .collect()
    }

    pub fn send_getheaders_to_peer(
        &self,
        nc: &Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        block_number_and_hash: BlockNumberAndHash,
    ) {
        if let Some(last_time) = self
            .state()
            .pending_get_headers
            .write()
            .get(&(peer, block_number_and_hash.hash()))
        {
            if Instant::now() < *last_time + GET_HEADERS_TIMEOUT {
                debug!(
                    "Last get_headers request to peer {} is less than {:?}; Ignore it.",
                    peer, GET_HEADERS_TIMEOUT,
                );
                return;
            } else {
                debug!(
                    "Can not get headers from {} in {:?}, retry",
                    peer, GET_HEADERS_TIMEOUT,
                );
            }
        }
        self.state()
            .pending_get_headers
            .write()
            .put((peer, block_number_and_hash.hash()), Instant::now());

        debug!(
            "send_getheaders_to_peer peer={}, hash={}",
            peer,
            block_number_and_hash.hash()
        );
        let locator_hash = self.get_locator(block_number_and_hash);
        let content = packed::GetHeaders::new_builder()
            .block_locator_hashes(locator_hash)
            .hash_stop(packed::Byte32::zero())
            .build();
        let message = packed::SyncMessage::new_builder().set(content).build();
        let nc = Arc::clone(nc);
        self.shared().async_handle().spawn(async move {
            async_send_message(SupportProtocols::Sync.protocol_id(), &nc, peer, &message).await
        });
    }

    pub fn get_block_status(&self, block_hash: &Byte32) -> BlockStatus {
        self.shared().get_block_status(block_hash)
    }

    pub fn contains_block_status(&self, block_hash: &Byte32, status: BlockStatus) -> bool {
        self.get_block_status(block_hash).contains(status)
    }
}
