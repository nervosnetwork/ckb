use bigint::H256;
use channel::Sender;
use ckb_core::block::Block;
use ckb_util::RwLockUpgradableReadGuard;
use jsonrpc::client::Client as JsonRpcClient;
use std::{thread, time};
use types::{BlockTemplate, Config, Shared};

#[derive(Clone)]
pub struct Client {
    pub shared: Shared,
    pub new_job_tx: Sender<()>,
    pub config: Config,
}

impl Client {
    pub fn run(&self) {
        self.poll_block_template();
    }

    fn poll_block_template(&self) {
        let client = JsonRpcClient::new(self.config.rpc_url.to_owned(), None, None);
        let method = "get_block_template";
        let params = [
            json!(self.config.type_hash),
            json!(self.config.max_transactions),
            json!(self.config.max_proposals),
        ];
        let request = client.build_request(method, &params);

        loop {
            debug!(target: "miner", "poll block template...");
            match client
                .send_request(&request)
                .and_then(|res| res.into_result::<BlockTemplate>())
            {
                Ok(new) => {
                    let is_new_job = |new: &BlockTemplate, old: &BlockTemplate| {
                        new.raw_header.number() != old.raw_header.number()
                            || new.commit_transactions.len()
                                >= old.commit_transactions.len()
                                    + self.config.new_transactions_threshold as usize
                    };
                    let inner = self.shared.inner.upgradable_read();
                    if inner.as_ref().map_or(true, |old| is_new_job(&new, &old)) {
                        let mut write_guard = RwLockUpgradableReadGuard::upgrade(inner);
                        *write_guard = Some(new);
                        self.new_job_tx.send(());
                    }
                }
                Err(e) => {
                    error!(target: "miner", "rpc call get_block_template error: {:?}", e);
                }
            }
            thread::sleep(time::Duration::from_secs(self.config.poll_interval));
        }
    }

    pub fn submit_block(&self, block: &Block) {
        let client = JsonRpcClient::new(self.config.rpc_url.to_owned(), None, None);
        let method = "submit_block";
        let params = [json!(block)];
        let request = client.build_request(method, &params);

        match client
            .send_request(&request)
            .and_then(|res| res.into_result::<H256>())
        {
            Ok(_) => {
                info!(target: "miner", "success to submit block #{}", block.header().number());
            }
            Err(e) => {
                error!(target: "miner", "rpc call submit_block error: {:?}", e);
            }
        }
    }
}
