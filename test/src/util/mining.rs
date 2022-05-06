use crate::util::chain::forward_main_blocks;
use crate::Node;
use crate::DEFAULT_TX_PROPOSAL_WINDOW;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_types::{
    core::{BlockBuilder, BlockView, EpochNumberWithFraction, HeaderView},
    packed,
};
use std::{thread::sleep, time::Duration};

pub fn out_bootstrap_period(nodes: &[Node]) {
    if let Some(node0) = nodes.first() {
        node0.mine_until_out_bootstrap_period();
        if nodes.len() <= 1 {
            return;
        }

        let tip_number = node0.get_tip_block_number();
        let range = 1..tip_number + 1;
        for node in nodes.iter().skip(1) {
            forward_main_blocks(node0, node, range.clone());
        }
    }
}

pub fn out_ibd_mode(nodes: &[Node]) {
    if let Some(node0) = nodes.first() {
        node0.mine_until_out_ibd_mode();
        if nodes.len() <= 1 {
            return;
        }

        let tip_number = node0.get_tip_block_number();
        let range = 1..tip_number + 1;
        for node in nodes.iter().skip(1) {
            forward_main_blocks(node0, node, range.clone());
        }
    }
}

impl Node {
    pub fn mine_until_out_ibd_mode(&self) {
        self.mine_until_bool(|| self.get_tip_block_number() > 0)
    }

    /// The `[1, PROPOSAL_WINDOW.farthest()]` of chain is called as bootstrap period. Cellbases w
    /// this period are zero capacity.
    ///
    /// This function will generate blank blocks until node.tip_block_number > PROPOSAL_WINDOW.fa
    ///
    /// Typically involve this function at the beginning of test.
    pub fn mine_until_out_bootstrap_period(&self) {
        // TODO predicate by output.is_some() is more realistic. But keeps original behaviours,
        // update it later.
        // let predicate = || {
        //     node.get_tip_block()
        //         .transaction(0)
        //         .map(|tx| tx.output(0).is_some())
        //         .unwrap_or(false)
        // };

        let farthest = self.consensus().tx_proposal_window().farthest();
        let out_bootstrap_period = farthest + 2;
        let predicate = || self.get_tip_block_number() >= out_bootstrap_period;
        self.mine_until_bool(predicate)
    }

    pub fn mine_until_epoch(&self, number: u64, index: u64, length: u64) {
        let target_epoch = EpochNumberWithFraction::new(number, index, length);
        self.mine_until_bool(|| {
            let tip_header: HeaderView = self.rpc_client().get_tip_header().into();
            let tip_epoch = tip_header.epoch();
            if tip_epoch > target_epoch {
                panic!(
                    "expect mine until epoch {} but already be epoch {}",
                    target_epoch, tip_epoch
                );
            }
            tip_epoch == target_epoch
        });
    }

    pub fn mine(&self, count: u64) {
        let with = |builder: BlockBuilder| builder.build();
        self.mine_with(count, with)
    }

    pub fn mine_with_blocking<B>(&self, blocking: B) -> u64
    where
        B: Fn(&mut BlockTemplate) -> bool,
    {
        let mut count = 0;
        let mut template = self.rpc_client().get_block_template(None, None, None);
        while blocking(&mut template) {
            sleep(Duration::from_millis(100));
            template = self.rpc_client().get_block_template(None, None, None);
            count += 1;

            if count > 300 {
                panic!("mine_with_blocking timeout");
            }
        }
        let block = packed::Block::from(template).as_advanced_builder().build();
        let number = block.number();
        self.rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap();
        number
    }

    pub fn mine_until_transactions_confirm_with_windows(&self, closest: u64) {
        let last = self.mine_with_blocking(|template| template.proposals.is_empty());
        self.mine_with_blocking(|template| template.number.value() != (last + closest - 1));
        self.mine_with_blocking(|template| template.transactions.is_empty());
    }

    pub fn mine_until_transactions_confirm(&self) {
        self.mine_until_transactions_confirm_with_windows(DEFAULT_TX_PROPOSAL_WINDOW.0)
    }

    pub fn mine_with<W>(&self, count: u64, with: W)
    where
        W: Fn(BlockBuilder) -> BlockView,
    {
        for _ in 0..count {
            let template = self.rpc_client().get_block_template(None, None, None);
            let builder = packed::Block::from(template).as_advanced_builder();
            let block = with(builder);
            self.rpc_client()
                .submit_block("".to_owned(), block.data().into())
                .unwrap();
            self.new_block_with_blocking(|template| {
                template.number.value() != (block.number() + 1)
            });
        }
    }

    pub fn mine_until_bool<P>(&self, predicate: P)
    where
        P: Fn() -> bool,
    {
        let until = || if predicate() { Some(()) } else { None };
        self.mine_until(until)
    }

    pub fn mine_until<T, U>(&self, until: U) -> T
    where
        U: Fn() -> Option<T>,
    {
        let with = |builder: BlockBuilder| builder.build();
        self.mine_until_with(until, with)
    }

    pub fn mine_until_with<W, T, U>(&self, until: U, with: W) -> T
    where
        U: Fn() -> Option<T>,
        W: Fn(BlockBuilder) -> BlockView,
    {
        loop {
            if let Some(t) = until() {
                return t;
            }

            let template = self.rpc_client().get_block_template(None, None, None);
            let builder = packed::Block::from(template).as_advanced_builder();
            let block = with(builder);
            self.rpc_client()
                .submit_block("".to_owned(), block.data().into())
                .unwrap();
            self.new_block_with_blocking(|template| {
                template.number.value() != (block.number() + 1)
            });
        }
    }
}
