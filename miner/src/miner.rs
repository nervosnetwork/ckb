use super::build_block_template;
use super::Config;
use block_template::BlockTemplate;
use chain::chain::ChainProvider;
use chain::PowEngine;
use ckb_notify::{Event, Notify, MINER_SUBSCRIBER};
use ckb_protocol::Payload;
use core::block::IndexedBlock;
use core::header::{RawHeader, Seal};
use core::transaction::ProposalTransaction;
use core::BlockNumber;
use crossbeam_channel;
use fnv::FnvHashSet;
use network::NetworkContextExt;
use network::NetworkService;
use pool::TransactionPool;
use rand::{thread_rng, Rng};
use std::collections::HashSet;
use std::sync::Arc;
use sync::compact_block::CompactBlockBuilder;
use sync::RELAY_PROTOCOL_ID;

pub struct Miner<C, P> {
    config: Config,
    chain: Arc<C>,
    pow: Arc<P>,
    network: Arc<NetworkService>,
    tx_pool: Arc<TransactionPool<C>>,
    sub_rx: crossbeam_channel::Receiver<Event>,
    mining_number: BlockNumber,
}

impl<C, P> Miner<C, P>
where
    C: ChainProvider + 'static,
    P: PowEngine + 'static,
{
    pub fn new(
        config: Config,
        chain: Arc<C>,
        pow: &Arc<P>,
        tx_pool: &Arc<TransactionPool<C>>,
        network: &Arc<NetworkService>,
        notify: &Notify,
    ) -> Self {
        let number = chain.tip_header().read().header.number;

        let (sub_tx, sub_rx) = crossbeam_channel::unbounded();
        notify.register_transaction_subscriber(MINER_SUBSCRIBER, sub_tx.clone());
        notify.register_tip_subscriber(MINER_SUBSCRIBER, sub_tx);

        Miner {
            config,
            chain,
            sub_rx,
            pow: Arc::clone(pow),
            tx_pool: Arc::clone(tx_pool),
            network: Arc::clone(network),
            mining_number: number,
        }
    }

    pub fn start(&mut self) {
        self.pow.init(self.mining_number);

        loop {
            self.commit_new_block();
        }
    }

    fn commit_new_block(&mut self) {
        match build_block_template(&self.chain, &self.tx_pool, self.config.redeem_script_hash) {
            Ok(block_template) => {
                self.mining_number = block_template.raw_header.number;
                if let Some((block, propasal)) = self.mine(block_template) {
                    debug!(target: "miner", "new block mined: {} -> (number: {}, difficulty: {}, timestamp: {})",
                          block.hash(), block.header.number, block.header.difficulty, block.header.timestamp);
                    if self.chain.process_block(&block).is_ok() {
                        self.tx_pool.proposal_n(block.number(), propasal);
                        self.announce_new_block(&block);
                    }
                }
            }
            Err(err) => {
                error!(target: "miner", "build_block_template: {:?}", err);
            }
        }
    }

    fn mine(
        &self,
        block_template: BlockTemplate,
    ) -> Option<(IndexedBlock, FnvHashSet<ProposalTransaction>)> {
        let BlockTemplate {
            raw_header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block_template;

        self.mine_loop(&raw_header).map(|seal| {
            (
                IndexedBlock {
                    header: raw_header.with_seal(seal).into(),
                    uncles,
                    commit_transactions,
                    proposal_transactions: proposal_transactions
                        .iter()
                        .map(|p| p.proposal_short_id())
                        .collect(),
                },
                proposal_transactions,
            )
        })
    }

    fn mine_loop(&self, header: &RawHeader) -> Option<Seal> {
        let new_transactions_threshold = self.config.new_transactions_threshold;
        let mut new_transactions_counter = 0;
        let mut nonce: u64 = thread_rng().gen();
        loop {
            debug!(target: "miner", "mining {}", nonce);
            match self.sub_rx.try_recv() {
                Some(Event::NewTip(block)) => {
                    if block.header.number >= self.mining_number {
                        break None;
                    }
                }
                Some(Event::NewTransaction) => {
                    if new_transactions_counter >= new_transactions_threshold {
                        break None;
                    } else {
                        new_transactions_counter += 1;
                    }
                }
                None => {}
                event => {
                    debug!(target: "miner", "Unexpected sub message {:?}", event);
                }
            }
            if let Some(seal) = self.pow.solve_header(header, nonce) {
                break Some(seal);
            }
            nonce = nonce.wrapping_add(1);
        }
    }

    fn announce_new_block(&self, block: &IndexedBlock) {
        self.network.with_context_eval(RELAY_PROTOCOL_ID, |nc| {
            for (peer_id, _session) in nc.sessions(&self.network.connected_peers()) {
                debug!(target: "miner", "announce new block to peer#{:?}, {} => {}",
                       peer_id, block.header().number, block.hash());
                let mut payload = Payload::new();
                let compact_block = CompactBlockBuilder::new(block, &HashSet::new()).build();
                payload.set_compact_block(compact_block.into());
                let _ = nc.send_payload(peer_id, payload);
            }
        });
    }
}
