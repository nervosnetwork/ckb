use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ckb_core::{header::Header, script::Script};
use ckb_jsonrpc_types::{BlockNumber, BlockView, ChainInfo, Node, TxPoolInfo};
use ckb_util::RwLock;
use jsonrpc_client_core::Error as RpcError;

use super::util::ts_now;
use ckb_sdk::HttpRpcClient;

const MAX_SAVE_BLOCKS: usize = 100;

pub fn start_rpc_thread(url: String, state: Arc<RwLock<State>>) {
    let mut rpc_client = HttpRpcClient::from_uri(url.as_str());
    thread::spawn(move || {
        while let Err(err) = process(&state, &mut rpc_client) {
            log::info!(
                "Load state error: {}, retry 2 seconds later",
                err.to_string()
            );
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn process(state: &Arc<RwLock<State>>, rpc_client: &mut HttpRpcClient) -> Result<(), RpcError> {
    loop {
        let chain_info_opt = rpc_client.get_blockchain_info().call().ok();
        let local_node_info = rpc_client.local_node_info().call()?;
        let tx_pool_info = rpc_client.tx_pool_info().call()?;
        let peers = rpc_client.get_peers().call()?;
        let tip_header: Header = rpc_client.get_tip_header().call()?.into();
        let new_block = {
            if state
                .read()
                .tip_header
                .as_ref()
                .map(|header| header.hash() != tip_header.hash())
                .unwrap_or(true)
            {
                rpc_client
                    .get_block(tip_header.hash().clone())
                    .call()
                    .unwrap()
                    .0
            } else {
                None
            }
        };
        {
            let mut state_mut = state.write();
            state_mut.chain = chain_info_opt;
            state_mut.tip_header = Some(tip_header.clone());
            state_mut.local_node = Some(local_node_info);
            state_mut.tx_pool = Some(tx_pool_info);
            state_mut.peers = peers.0;

            // Handle fork
            if let Some(last_block) = state_mut.blocks.values().rev().next() {
                let last_hash = last_block.header.hash();
                if tip_header.parent_hash() != last_hash && tip_header.hash() != last_hash {
                    state_mut.blocks.clear();
                }
            }

            // Insert tip block
            if let Some(block) = new_block {
                let number = block.header.inner.number.0;
                state_mut.blocks.insert(number, block.into());
            }

            if state_mut
                .chain
                .as_ref()
                .map(|chain| !chain.is_initial_block_download)
                .unwrap_or(false)
            {
                // Handle init and fork
                while state_mut.blocks.len() < MAX_SAVE_BLOCKS {
                    let first_number = state_mut.blocks.keys().next().cloned().unwrap();
                    if first_number < 1 {
                        break;
                    }
                    if let Some(block) = rpc_client
                        .get_block_by_number(BlockNumber(first_number - 1))
                        .call()?
                        .0
                    {
                        state_mut.blocks.insert(first_number - 1, block.into());
                    } else {
                        break;
                    }
                }
                // Remove old blocks
                while state_mut.blocks.len() > MAX_SAVE_BLOCKS {
                    let first_number = state_mut.blocks.keys().next().cloned().unwrap();
                    state_mut.blocks.remove(&first_number);
                }
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

#[derive(Default)]
pub struct State {
    // FIXME: should handle fork (see: ckb-monitor)
    pub(crate) blocks: BTreeMap<u64, BlockInfo>,
    pub(crate) tip_header: Option<Header>,
    pub(crate) peers: Vec<Node>,
    pub(crate) chain: Option<ChainInfo>,
    pub(crate) local_node: Option<Node>,
    pub(crate) tx_pool: Option<TxPoolInfo>,
}

impl State {
    pub fn summary(&self) -> SummaryInfo {
        SummaryInfo {
            tip: self.blocks.values().last().cloned(),
            chain: self.chain.as_ref().map(|info| ChainInfo {
                chain: info.chain.clone(),
                median_time: info.median_time.clone(),
                epoch: info.epoch.clone(),
                difficulty: info.difficulty.clone(),
                is_initial_block_download: info.is_initial_block_download,
                alerts: info.alerts.clone(),
            }),
            tx_pool: self.tx_pool.clone(),
            peer_count: self.peers.len(),
        }
    }
}

pub struct SummaryInfo {
    pub(crate) chain: Option<ChainInfo>,
    pub(crate) tip: Option<BlockInfo>,
    pub(crate) tx_pool: Option<TxPoolInfo>,
    pub(crate) peer_count: usize,
}

#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub(crate) header: Header,
    pub(crate) got_at: u64,
    pub(crate) uncle_count: usize,
    pub(crate) commit_tx_count: usize,
    pub(crate) proposal_tx_count: usize,
    pub(crate) input_count: usize,
    pub(crate) output_count: usize,
    pub(crate) cellbase_outputs: Vec<(u64, Script)>,
}

impl From<BlockView> for BlockInfo {
    fn from(view: BlockView) -> BlockInfo {
        let header = view.header.into();
        let uncle_count = view.uncles.len();
        let commit_tx_count = view.transactions.len();
        let proposal_tx_count = view.proposals.len();
        let cellbase = &view.transactions[0].inner;
        let cellbase_outputs = cellbase
            .outputs
            .iter()
            .map(|output| (output.capacity.0.as_u64(), output.lock.clone().into()))
            .collect::<Vec<(u64, Script)>>();
        let mut input_count = 0;
        let mut output_count = 0;
        for tx in &view.transactions {
            input_count += tx.inner.inputs.len();
            output_count += tx.inner.outputs.len();
        }
        let got_at = ts_now();
        BlockInfo {
            header,
            got_at,
            uncle_count,
            commit_tx_count,
            proposal_tx_count,
            input_count,
            output_count,
            cellbase_outputs,
        }
    }
}
