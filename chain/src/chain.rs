use crate::error::ProcessBlockError;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::BlockExt;
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE};
use ckb_core::transaction::Transaction;
use ckb_notify::{BlockCategory, Forks, NotifyController, NotifyService};
use ckb_shared::error::SharedError;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared, TipHeader};
use ckb_verification::{BlockVerifier, TransactionsVerifier, Verifier};
use crossbeam_channel::{self, select, Receiver, Sender};
use faketime::unix_time_as_millis;
use log::{self, debug, error};
use numext_fixed_hash::H256;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub struct ChainService<CI> {
    shared: Shared<CI>,
    notify: NotifyController,
    block_verifier: BlockVerifier<Shared<CI>>,
}

#[derive(Clone)]
pub struct ChainController {
    process_block_sender: Sender<Request<Arc<Block>, Result<(), ProcessBlockError>>>,
}

pub struct ChainReceivers {
    process_block_receiver: Receiver<Request<Arc<Block>, Result<(), ProcessBlockError>>>,
}

impl ChainController {
    pub fn build() -> (ChainController, ChainReceivers) {
        let (process_block_sender, process_block_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        (
            ChainController {
                process_block_sender,
            },
            ChainReceivers {
                process_block_receiver,
            },
        )
    }

    pub fn process_block(&self, block: Arc<Block>) -> Result<(), ProcessBlockError> {
        Request::call(&self.process_block_sender, block).expect("process_block() failed")
    }
}

impl<CI: ChainIndex + 'static> ChainService<CI> {
    pub fn new(shared: Shared<CI>, notify: NotifyController) -> ChainService<CI> {
        let block_verifier = BlockVerifier::new(shared.clone());
        ChainService {
            shared,
            notify,
            block_verifier,
        }
    }

    pub fn start<S: ToString>(
        mut self,
        thread_name: Option<S>,
        receivers: ChainReceivers,
    ) -> JoinHandle<()> {
        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        thread_builder
            .spawn(move || loop {
                select! {
                    recv(receivers.process_block_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: block }) => {
                            let _ = responder.send(self.process_block(&block));
                        },
                        _ => {
                            error!(target: "chain", "process_block_receiver closed");
                            break;
                        },
                    }
                }
            })
            .expect("Start ChainService failed")
    }

    fn process_block(&mut self, block: &Block) -> Result<(), ProcessBlockError> {
        debug!(target: "chain", "begin processing block: {}", block.header().hash());
        if self.shared.consensus().verification {
            self.block_verifier
                .verify(&block)
                .map_err(ProcessBlockError::Verification)?
        }

        let block_category = self
            .insert_block(&block)
            .map_err(ProcessBlockError::Shared)?;

        self.notify.notify_new_block(block_category);
        debug!(target: "chain", "finish processing block");
        Ok(())
    }

    fn insert_block(&self, block: &Block) -> Result<BlockCategory, SharedError> {
        let block_hash = block.header().hash();
        let parent_ext = self
            .shared
            .store()
            .get_block_ext(&block.header().parent_hash())
            .expect("parent already store");

        let block_ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: parent_ext.total_difficulty + block.header().difficulty(),
            total_uncles_count: parent_ext.total_uncles_count + block.uncles().len() as u64,
            commit_transactions_validated: None,
        };

        // save block and block_ext
        self.shared.store().save_with_batch(|batch| {
            self.shared.store().insert_block(batch, block);
            self.shared
                .store()
                .insert_block_ext(batch, &block_hash, &block_ext);
            Ok(())
        })?;

        let tip = self.shared.tip();

        let txs_verify_fn = |transactions: &[Transaction]| {
            if self.shared.consensus().verification {
                let transactions_verifier = TransactionsVerifier::new(self.shared.clone());
                transactions_verifier.verify(transactions).is_ok()
            } else {
                true
            }
        };

        // block is being added to main branch, forward it.
        if &tip.hash == block.header().parent_hash() {
            self.shared.store().forward(&block_hash, txs_verify_fn)?;
            return Ok(BlockCategory::MainBranch(block_hash));
        }

        // block is being added to side branch, do nothing.
        if tip.total_difficulty > block_ext.total_difficulty
            || (tip.total_difficulty == block_ext.total_difficulty && block_hash >= tip.hash)
        {
            return Ok(BlockCategory::SideBranch(block_hash));
        }

        // block is being added to side branch and switched to main branch.
        let forks = self.get_forks(&block_hash);

        (forks.ancestor..tip.number)
            .map(|_| self.shared.store().rollback())
            .collect::<Result<Vec<_>, _>>()?;

        forks
            .side_blocks
            .iter()
            .map(|hash| self.shared.store().forward(hash, txs_verify_fn))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(BlockCategory::SideSwitchToMain(forks))
    }

    fn get_forks(&self, hash: &H256) -> Forks {
        let mut side_blocks = Vec::new();
        let mut hash = hash.clone();
        loop {
            match self.shared.store().get_block_number(&hash) {
                Some(number) => {
                    side_blocks.reverse();
                    let tip = self.shared.store().get_tip().read();
                    return Forks {
                        ancestor: number,
                        side_blocks,
                        main_blocks: (number + 1..=tip.number)
                            .map(|n| {
                                self.shared
                                    .store()
                                    .get_block_hash(n)
                                    .expect("block already store")
                            })
                            .collect(),
                    };
                }
                None => {
                    side_blocks.push(hash.clone());
                    hash = self
                        .shared
                        .store()
                        .get_header(&hash)
                        .expect("parent already store")
                        .parent_hash()
                        .clone();
                }
            }
        }
    }
}

pub struct ChainBuilder<CI> {
    shared: Shared<CI>,
    notify: Option<NotifyController>,
}

impl<CI: ChainIndex + 'static> ChainBuilder<CI> {
    pub fn new(shared: Shared<CI>) -> ChainBuilder<CI> {
        let mut consensus = Consensus::default();
        consensus.initial_block_reward = 50;
        ChainBuilder {
            shared,
            notify: None,
        }
    }

    pub fn notify(mut self, value: NotifyController) -> Self {
        self.notify = Some(value);
        self
    }

    pub fn build(mut self) -> ChainService<CI> {
        let notify = self.notify.take().unwrap_or_else(|| {
            // FIXME: notify should not be optional
            let (_handle, notify) = NotifyService::default().start::<&str>(None);
            notify
        });
        ChainService::new(self.shared, notify)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use ckb_core::block::BlockBuilder;
    use ckb_core::cell::CellProvider;
    use ckb_core::header::{BlockNumber, Header, HeaderBuilder};
    use ckb_core::transaction::{
        CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
    };
    use ckb_core::uncle::UncleBlock;
    use ckb_db::memorydb::MemoryKeyValueDB;
    use ckb_shared::shared::SharedBuilder;
    use ckb_shared::store::{ChainKVStore, ChainStore};
    use numext_fixed_uint::U256;

    fn start_chain(
        consensus: Option<Consensus>,
    ) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
        let builder = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory();
        let shared = builder
            .consensus(consensus.unwrap_or_else(|| Consensus::default().set_verification(false)))
            .build();

        let (chain_controller, chain_receivers) = ChainController::build();
        let chain_service = ChainBuilder::new(shared.clone()).build();
        let _handle = chain_service.start::<&str>(None, chain_receivers);
        (chain_controller, shared)
    }

    fn create_cellbase(number: BlockNumber) -> Transaction {
        TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .output(CellOutput::new(0, vec![], H256::zero(), None))
            .build()
    }

    fn gen_block(
        parent_header: &Header,
        nonce: u64,
        difficulty: U256,
        commit_transactions: Vec<Transaction>,
        uncles: Vec<UncleBlock>,
    ) -> Block {
        let number = parent_header.number() + 1;
        let cellbase = create_cellbase(number);
        let header = HeaderBuilder::default()
            .parent_hash(parent_header.hash().clone())
            .timestamp(unix_time_as_millis())
            .number(number)
            .difficulty(difficulty)
            .nonce(nonce)
            .build();

        BlockBuilder::default()
            .header(header)
            .commit_transaction(cellbase)
            .commit_transactions(commit_transactions)
            .uncles(uncles)
            .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
            .build()
    }

    fn create_transaction(parent: H256) -> Transaction {
        let mut output = CellOutput::default();
        output.capacity = 100_000_000 / 100 as u64;
        let outputs: Vec<CellOutput> = vec![output.clone(); 100];

        TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(parent, 0), Default::default()))
            .outputs(outputs)
            .build()
    }

    #[test]
    fn test_genesis_transaction_spend() {
        let tx = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), Default::default()))
            .outputs(vec![
                CellOutput::new(
                    100_000_000,
                    vec![],
                    H256::default(),
                    None
                );
                100
            ])
            .build();

        let mut root_hash = tx.hash().clone();

        let genesis_block = BlockBuilder::default()
            .commit_transaction(tx)
            .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

        let consensus = Consensus::default()
            .set_genesis_block(genesis_block)
            .set_verification(false);
        let (chain_controller, shared) = start_chain(Some(consensus));

        let end = 21;

        let mut blocks1: Vec<Block> = vec![];
        let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..end {
            let difficulty = parent.difficulty().clone();
            let tx = create_transaction(root_hash);
            root_hash = tx.hash().clone();
            let new_block = gen_block(&parent, i, difficulty + U256::from(1u64), vec![tx], vec![]);
            blocks1.push(new_block.clone());
            parent = new_block.header().clone();
        }

        for block in &blocks1[0..10] {
            assert!(chain_controller
                .process_block(Arc::new(block.clone()))
                .is_ok());
        }
    }

    #[test]
    fn test_genesis_transaction_fetch() {
        let tx = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), Default::default()))
            .outputs(vec![
                CellOutput::new(
                    100_000_000,
                    vec![],
                    H256::default(),
                    None
                );
                100
            ])
            .build();

        let root_hash = tx.hash().clone();

        let genesis_block = BlockBuilder::default()
            .commit_transaction(tx)
            .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

        let consensus = Consensus::default()
            .set_genesis_block(genesis_block)
            .set_verification(false);
        let (_chain_controller, shared) = start_chain(Some(consensus));

        let out_point = OutPoint::new(root_hash, 0);
        let state = shared.cell(&out_point);
        assert!(state.is_live());
    }

    #[test]
    fn test_chain_fork_by_total_difficulty() {
        let (chain_controller, shared) = start_chain(None);
        let final_number = 20;

        let mut chain1: Vec<Block> = Vec::new();
        let mut chain2: Vec<Block> = Vec::new();

        let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty().clone();
            let new_block = gen_block(&parent, i, difficulty + U256::from(100u64), vec![], vec![]);
            chain1.push(new_block.clone());
            parent = new_block.header().clone();
        }

        parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty().clone();
            let j = if i > 10 { 110 } else { 99 };
            let new_block = gen_block(
                &parent,
                i + 1000,
                difficulty + U256::from(j as u32),
                vec![],
                vec![],
            );
            chain2.push(new_block.clone());
            parent = new_block.header().clone();
        }

        for block in &chain1 {
            chain_controller
                .process_block(Arc::new(block.clone()))
                .expect("process block ok");
        }

        for block in &chain2 {
            chain_controller
                .process_block(Arc::new(block.clone()))
                .expect("process block ok");
        }
        assert_eq!(
            shared.block_hash(8),
            chain2.get(7).map(|b| b.header().hash())
        );
    }

    #[test]
    fn test_chain_fork_by_hash() {
        let (chain_controller, shared) = start_chain(None);
        let final_number = 20;

        let mut chain1: Vec<Block> = Vec::new();
        let mut chain2: Vec<Block> = Vec::new();

        let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty().clone();
            let new_block = gen_block(&parent, i, difficulty + U256::from(100u64), vec![], vec![]);
            chain1.push(new_block.clone());
            parent = new_block.header().clone();
        }

        parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty().clone();
            let new_block = gen_block(
                &parent,
                i + 1000,
                difficulty + U256::from(100u64),
                vec![],
                vec![],
            );
            chain2.push(new_block.clone());
            parent = new_block.header().clone();
        }

        for block in &chain1 {
            chain_controller
                .process_block(Arc::new(block.clone()))
                .expect("process block ok");
        }

        for block in &chain2 {
            chain_controller
                .process_block(Arc::new(block.clone()))
                .expect("process block ok");
        }

        //if total_difficulty equal, we chose block which have smaller hash as best
        assert!(chain1
            .iter()
            .zip(chain2.iter())
            .all(|(a, b)| a.header().difficulty() == b.header().difficulty()));

        let best = if chain1[(final_number - 2) as usize].header().hash()
            < chain2[(final_number - 2) as usize].header().hash()
        {
            chain1
        } else {
            chain2
        };
        assert_eq!(shared.block_hash(8), best.get(7).map(|b| b.header().hash()));
        assert_eq!(
            shared.block_hash(19),
            best.get(18).map(|b| b.header().hash())
        );
    }

    #[test]
    fn test_chain_get_ancestor() {
        let (chain_controller, shared) = start_chain(None);
        let final_number = 20;

        let mut chain1: Vec<Block> = Vec::new();
        let mut chain2: Vec<Block> = Vec::new();

        let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty().clone();
            let new_block = gen_block(&parent, i, difficulty + U256::from(100u64), vec![], vec![]);
            chain1.push(new_block.clone());
            parent = new_block.header().clone();
        }

        parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty().clone();
            let new_block = gen_block(
                &parent,
                i + 1000,
                difficulty + U256::from(100u64),
                vec![],
                vec![],
            );
            chain2.push(new_block.clone());
            parent = new_block.header().clone();
        }

        for block in &chain1 {
            chain_controller
                .process_block(Arc::new(block.clone()))
                .expect("process block ok");
        }

        for block in &chain2 {
            chain_controller
                .process_block(Arc::new(block.clone()))
                .expect("process block ok");
        }

        assert_eq!(
            *chain1[9].header(),
            shared
                .get_ancestor(&chain1.last().unwrap().header().hash(), 10)
                .unwrap()
        );

        assert_eq!(
            *chain2[9].header(),
            shared
                .get_ancestor(&chain2.last().unwrap().header().hash(), 10)
                .unwrap()
        );
    }

    #[test]
    fn test_calculate_difficulty() {
        let genesis_block = BlockBuilder::default()
            .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));
        let mut consensus = Consensus::default()
            .set_genesis_block(genesis_block)
            .set_verification(false);
        consensus.pow_time_span = 200;
        consensus.pow_spacing = 1;

        let (chain_controller, shared) = start_chain(Some(consensus.clone()));
        let final_number = shared.consensus().difficulty_adjustment_interval();

        let mut chain1: Vec<Block> = Vec::new();
        let mut chain2: Vec<Block> = Vec::new();

        let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number - 1 {
            let difficulty = shared.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(&parent, i, difficulty, vec![], vec![]);
            chain_controller
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            chain1.push(new_block.clone());
            parent = new_block.header().clone();
        }

        parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = shared.calculate_difficulty(&parent).unwrap();
            let mut uncles = vec![];
            if i < 26 {
                uncles.push(chain1[i as usize].clone().into());
            }
            let new_block = gen_block(&parent, i + 100, difficulty, vec![], uncles);
            chain_controller
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header().clone();
        }
        let tip = shared
            .store()
            .get_header(&shared.store().get_tip().read().hash)
            .unwrap();
        let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 25);
        let difficulty = shared.calculate_difficulty(&tip).unwrap();

        // 25 * 10 * 1000 / 200
        assert_eq!(difficulty, U256::from(1250u64));

        let (chain_controller, shared) = start_chain(Some(consensus.clone()));
        let mut chain2: Vec<Block> = Vec::new();
        for i in 1..final_number - 1 {
            chain_controller
                .process_block(Arc::new(chain1[(i - 1) as usize].clone()))
                .expect("process block ok");
        }

        parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = shared.calculate_difficulty(&parent).unwrap();
            let mut uncles = vec![];
            if i < 11 {
                uncles.push(chain1[i as usize].clone().into());
            }
            let new_block = gen_block(&parent, i + 100, difficulty, vec![], uncles);
            chain_controller
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header().clone();
        }
        let tip = shared
            .store()
            .get_header(&shared.store().get_tip().read().hash)
            .unwrap();
        let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 10);
        let difficulty = shared.calculate_difficulty(&tip).unwrap();

        // min[10 * 10 * 1000 / 200, 1000]
        assert_eq!(difficulty, U256::from(1000u64));

        let (chain_controller, shared) = start_chain(Some(consensus.clone()));
        let mut chain2: Vec<Block> = Vec::new();
        for i in 1..final_number - 1 {
            chain_controller
                .process_block(Arc::new(chain1[(i - 1) as usize].clone()))
                .expect("process block ok");
        }

        parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = shared.calculate_difficulty(&parent).unwrap();
            let mut uncles = vec![];
            if i < 151 {
                uncles.push(chain1[i as usize].clone().into());
            }
            let new_block = gen_block(&parent, i + 100, difficulty, vec![], uncles);
            chain_controller
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header().clone();
        }
        let tip = shared
            .store()
            .get_header(&shared.store().get_tip().read().hash)
            .unwrap();
        let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 150);
        let difficulty = shared.calculate_difficulty(&tip).unwrap();

        // max[150 * 10 * 1000 / 200, 2 * 1000]
        assert_eq!(difficulty, U256::from(2000u64));
    }
}
