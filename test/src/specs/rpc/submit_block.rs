use crate::util::mining::{mine_until_epoch, mine_until_out_bootstrap_period};
use crate::utils::now_ms;
use crate::{Node, Spec};
use ckb_types::{
    core::{EpochNumberWithFraction, HeaderView},
    prelude::*,
};

pub struct RpcSubmitBlock;

impl Spec for RpcSubmitBlock {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        mine_until_out_bootstrap_period(node0);
        mine_until_out_bootstrap_period(node1);

        // build block with wrong block number
        let block = node0
            .new_block_builder(None, None, None)
            .number(2.pack())
            .build();
        let block_err = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap_err();
        assert!(
            block_err.to_string().contains("NumberError"),
            "Submit block with wrong block number should return 'NumberError'"
        );

        // build block with wrong block version
        let block = node0
            .new_block_builder(None, None, None)
            .version((42_u32).pack())
            .build();
        let block_err = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap_err();
        assert!(
            block_err.to_string().contains("BlockVersionError"),
            "Submit block with wrong block version should return 'BlockVersionError'"
        );

        // build block with wrong epoch
        let tip_header: HeaderView = node0.rpc_client().get_tip_header().into();
        let tip_epoch = tip_header.epoch();
        mine_until_epoch(
            node0,
            tip_epoch.number(),
            tip_epoch.length() - 1,
            tip_epoch.length(),
        );
        let epoch = EpochNumberWithFraction::new(tip_epoch.number() + 1, 0, 100);
        let block = node0
            .new_block_builder(None, None, None)
            .epoch(epoch.pack())
            .build();
        let block_err = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap_err();
        assert!(
            block_err.to_string().contains("Epoch(NumberMismatch"),
            "Submit block with wrong epoch should return 'EpochNumberMismatch"
        );

        // build block with wrong parent_hash: ancestor block hash
        let ancestor_block_hash = node0.get_block_by_number(1).hash();
        let block = node0
            .new_block_builder(None, None, None)
            .parent_hash(ancestor_block_hash)
            .build();
        let block_err = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap_err();
        assert!(
            block_err.to_string().contains("NumberError"),
            "Submit block with parent hash point to an ancestor should return 'NumberError'"
        );

        // build block with wrong parent_hash: unknown block hash
        let another_tip_block_hash = node1.get_tip_block().hash();
        let block = node0
            .new_block_builder(None, None, None)
            .parent_hash(another_tip_block_hash)
            .build();
        let block_err = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap_err();
        assert!(
            block_err.to_string().contains("UnknownParentError"),
            "Submit block with wrong parent hash should return 'UnknownParentError'"
        );

        // build block with wrong timestamp: block time too new
        // the limit of too new in ckb come from a const `ALLOWED_FUTURE_BLOCKTIME` which set as 15s
        // so here plus another 15s to make sure when submit the block it still out of the limit
        let block = node0
            .new_block_builder(None, None, None)
            .timestamp((now_ms() + 30_000).pack())
            .build();
        let block_err = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap_err();
        assert!(
            block_err.to_string().contains("BlockTimeTooNew"),
            "Submit block with future timestamp greater than 15s should return 'BlockTimeTooNew'"
        );

        // build block with wrong timestamp: block time too old
        let median_time = node0.rpc_client().get_blockchain_info().median_time.value();
        let block = node0
            .new_block_builder(None, None, None)
            .timestamp((median_time - 1).pack())
            .build();
        let block_err = node0
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap_err();
        assert!(
            block_err.to_string().contains("BlockTimeTooOld"),
            "Submit block with timestamp early than median time should return 'BlockTimeTooOld'"
        );
    }
}
