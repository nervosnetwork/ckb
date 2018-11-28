use bigint::H256;
use channel::{self, Receiver, Sender};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{Header, HeaderBuilder, RawHeader};
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE};
use ckb_core::transaction::{
    CellInput, CellOutput, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::uncle::UncleBlock;
use ckb_notify::{NotifyController, RPC_SUBSCRIBER};
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_shared::error::SharedError;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_time::now_ms;
use fnv::{FnvHashMap, FnvHashSet};
use std::cmp;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[derive(Serialize, Debug)]
pub struct BlockTemplate {
    pub raw_header: RawHeader,
    pub uncles: Vec<UncleBlock>,
    pub commit_transactions: Vec<Transaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

type BlockTemplateArgs = (H256, usize, usize);
type BlockTemplateReturn = Result<BlockTemplate, SharedError>;

#[derive(Clone)]
pub struct RpcController {
    get_block_template_sender: Sender<Request<BlockTemplateArgs, BlockTemplateReturn>>,
}

pub struct RpcReceivers {
    get_block_template_receiver: Receiver<Request<BlockTemplateArgs, BlockTemplateReturn>>,
}

// TODO: MinerService should dependent on RpcService
// To do this, we need to add follow api:
//   * get_block
//   * get_block_hash
//   * get_transaction
//   * send_transaction
//   * get_cells_by_type_hash
//   * submit_block
//   * receive notify
impl RpcController {
    pub fn new() -> (RpcController, RpcReceivers) {
        let (get_block_template_sender, get_block_template_receiver) =
            channel::bounded(DEFAULT_CHANNEL_SIZE);
        (
            RpcController {
                get_block_template_sender,
            },
            RpcReceivers {
                get_block_template_receiver,
            },
        )
    }

    pub fn get_block_template(
        &self,
        type_hash: H256,
        max_tx: usize,
        max_prop: usize,
    ) -> BlockTemplateReturn {
        Request::call(
            &self.get_block_template_sender,
            (type_hash, max_tx, max_prop),
        ).expect("get_block_template() failed")
    }
}

pub struct RpcService<CI> {
    shared: Shared<CI>,
    tx_pool: TransactionPoolController,
    candidate_uncles: FnvHashMap<H256, Arc<Block>>,
}

impl<CI: ChainIndex + 'static> RpcService<CI> {
    pub fn new(shared: Shared<CI>, tx_pool: TransactionPoolController) -> RpcService<CI> {
        RpcService {
            shared,
            tx_pool,
            candidate_uncles: FnvHashMap::default(),
        }
    }

    pub fn start<S: ToString>(
        mut self,
        thread_name: Option<S>,
        receivers: RpcReceivers,
        notify: &NotifyController,
    ) -> JoinHandle<()> {
        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let new_uncle_receiver = notify.subscribe_new_uncle(RPC_SUBSCRIBER);
        thread_builder
            .spawn(move || loop {
                select! {
                    recv(new_uncle_receiver, msg) => match msg {
                        Some(uncle_block) => {
                            let hash = uncle_block.header().hash();
                            self.candidate_uncles.insert(hash, uncle_block);
                        }
                        None => {
                            error!(target: "chain", "new_uncle_receiver closed");
                            break;
                        }
                    }
                    recv(receivers.get_block_template_receiver, msg) => match msg {
                        Some(Request { responder, arguments: (type_hash, max_tx, max_prop) }) => {
                            responder.send(self.get_block_template(type_hash, max_tx, max_prop));
                        },
                        None => {
                            error!(target: "chain", "get_block_template_receiver closed");
                            break;
                        },
                    }

                }
            }).expect("Start ChainService failed")
    }

    // TODO: the max size
    fn get_block_template(
        &mut self,
        type_hash: H256,
        max_tx: usize,
        max_prop: usize,
    ) -> BlockTemplateReturn {
        let (cellbase, commit_transactions, proposal_transactions, header_builder) = {
            let tip_header = self.shared.tip_header().read();
            let header = tip_header.inner();
            let now = cmp::max(now_ms(), header.timestamp() + 1);
            let difficulty = self
                .shared
                .calculate_difficulty(header)
                .expect("get difficulty");

            let (proposal_transactions, commit_transactions) = self
                .tx_pool
                .get_proposal_commit_transactions(max_prop, max_tx);

            let cellbase =
                self.create_cellbase_transaction(header, &commit_transactions, type_hash)?;

            let header_builder = HeaderBuilder::default()
                .parent_hash(&header.hash())
                .timestamp(now)
                .number(header.number() + 1)
                .difficulty(&difficulty)
                .cellbase_id(&cellbase.hash());
            (
                cellbase,
                commit_transactions,
                proposal_transactions,
                header_builder,
            )
        };

        let block = BlockBuilder::default()
            .commit_transaction(cellbase)
            .commit_transactions(commit_transactions)
            .proposal_transactions(proposal_transactions)
            .uncles(self.get_tip_uncles())
            .with_header_builder(header_builder);

        Ok(BlockTemplate {
            raw_header: block.header().clone().into_raw(),
            uncles: block.uncles().to_vec(),
            commit_transactions: block.commit_transactions().to_vec(),
            proposal_transactions: block.proposal_transactions().to_vec(),
        })
    }

    fn create_cellbase_transaction(
        &self,
        header: &Header,
        transactions: &[Transaction],
        type_hash: H256,
    ) -> Result<Transaction, SharedError> {
        // NOTE: To generate different cellbase txid, we put header number in the input script
        let input = CellInput::new_cellbase_input(header.number() + 1);
        // NOTE: We could've just used byteorder to serialize u64 and hex string into bytes,
        // but the truth is we will modify this after we designed lock script anyway, so let's
        // stick to the simpler way and just convert everything to a single string, then to UTF8
        // bytes, they really serve the same purpose at the moment
        let block_reward = self.shared.block_reward(header.number() + 1);
        let mut fee = 0;
        for transaction in transactions {
            fee += self.shared.calculate_transaction_fee(transaction)?;
        }

        let output = CellOutput::new(block_reward + fee, Vec::new(), type_hash, None);

        Ok(TransactionBuilder::default()
            .input(input)
            .output(output)
            .build())
    }

    fn get_tip_uncles(&mut self) -> Vec<UncleBlock> {
        let max_uncles_age = self.shared.consensus().max_uncles_age();
        let tip_header = self.shared.tip_header().read();
        let header = tip_header.inner();
        let mut excluded = FnvHashSet::default();

        // cB
        // tip      1 depth, valid uncle
        // tip.p^0  ---/  2
        // tip.p^1  -----/  3
        // tip.p^2  -------/  4
        // tip.p^3  ---------/  5
        // tip.p^4  -----------/  6
        // tip.p^5  -------------/
        // tip.p^6
        let mut block_hash = header.hash();
        excluded.insert(block_hash);
        for _depth in 0..max_uncles_age {
            if let Some(block) = self.shared.block(&block_hash) {
                excluded.insert(block.header().parent_hash());
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header().parent_hash();
            } else {
                break;
            }
        }

        let tip_difficulty_epoch =
            header.number() / self.shared.consensus().difficulty_adjustment_interval();

        let max_uncles_len = self.shared.consensus().max_uncles_len();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_len);
        let mut bad_uncles = Vec::new();
        let current_number = tip_header.number() + 1;
        for (hash, block) in &self.candidate_uncles {
            if uncles.len() == max_uncles_len {
                break;
            }

            let block_difficulty_epoch =
                block.header().number() / self.shared.consensus().difficulty_adjustment_interval();

            // uncle must be same difficulty epoch with tip
            if !block.header().difficulty() == header.difficulty()
                || !block_difficulty_epoch == tip_difficulty_epoch
            {
                bad_uncles.push(*hash);
                continue;
            }

            let depth = current_number.saturating_sub(block.header().number());
            if depth > max_uncles_age as u64
                || depth < 1
                || included.contains(hash)
                || excluded.contains(hash)
            {
                bad_uncles.push(*hash);
            } else if let Some(cellbase) = block.commit_transactions().first() {
                let uncle = UncleBlock {
                    header: block.header().clone(),
                    cellbase: cellbase.clone(),
                    proposal_transactions: block.proposal_transactions().to_vec(),
                };
                uncles.push(uncle);
                included.insert(*hash);
            } else {
                bad_uncles.push(*hash);
            }
        }

        if !bad_uncles.is_empty() {
            for bad in bad_uncles {
                self.candidate_uncles.remove(&bad);
            }
        }

        uncles
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use bigint::H256;
    use ckb_core::block::BlockBuilder;
    use ckb_db::memorydb::MemoryKeyValueDB;
    use ckb_notify::NotifyService;
    use ckb_pool::txs_pool::{PoolConfig, TransactionPoolController, TransactionPoolService};
    use ckb_shared::shared::SharedBuilder;
    use ckb_shared::store::ChainKVStore;
    use ckb_verification::{BlockVerifier, HeaderResolverWrapper, HeaderVerifier, Verifier};

    #[test]
    fn test_block_template() {
        let (_handle, notify) = NotifyService::default().start::<&str>(None);
        let (tx_pool_controller, tx_pool_receivers) = TransactionPoolController::new();
        let (rpc_controller, rpc_receivers) = RpcController::new();

        let shared = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory().build();
        let tx_pool_service =
            TransactionPoolService::new(PoolConfig::default(), shared.clone(), notify.clone());
        let _handle = tx_pool_service.start::<&str>(None, tx_pool_receivers);

        let rpc_service = RpcService::new(shared.clone(), tx_pool_controller.clone());
        let _handle = rpc_service.start(Some("RpcService"), rpc_receivers, &notify);

        let block_template = rpc_controller
            .get_block_template(H256::from(0), 1000, 1000)
            .unwrap();

        let BlockTemplate {
            raw_header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block_template;

        //do not verfiy pow here
        let header = raw_header.with_seal(Default::default());

        let block = BlockBuilder::default()
            .header(header)
            .uncles(uncles)
            .commit_transactions(commit_transactions)
            .proposal_transactions(proposal_transactions)
            .build();

        let resolver = HeaderResolverWrapper::new(block.header(), shared.clone());
        let header_verifier = HeaderVerifier::new(Arc::clone(&shared.consensus().pow_engine()));

        assert!(header_verifier.verify(&resolver).is_ok());

        let block_verfier = BlockVerifier::new(shared.clone());
        assert!(block_verfier.verify(&block).is_ok());
    }
}
