use crate::error::RPCError;
use ckb_chain::chain::ChainController;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{Block, BlockTemplate, Byte32, Transaction};
use ckb_logger::error;
use ckb_network::{NetworkController, SupportProtocols};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        self,
        cell::{
            resolve_transaction, OverlayCellProvider, ResolvedTransaction, TransactionsProvider,
        },
        BlockView,
    },
    packed,
    prelude::*,
    H256,
};
use ckb_verification_traits::Switch;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;
use std::sync::Arc;

/// RPC for Integration Test.
#[rpc(server)]
pub trait IntegrationTestRpc {
    /// process block without any block verification.
    ///
    /// ## Params
    ///
    /// * `data` - block data(in binary).
    ///
    /// * `broadcast` - true to enable broadcast(relay) the block to other peers.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "process_block_without_verify",
    ///   "params": [
    ///    {
    /// 	"header": {
    /// 		"compact_target": "0x1e083126",
    /// 		"dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    /// 		"epoch": "0x7080018000001",
    /// 		"extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    /// 		"nonce": "0x0",
    /// 		"number": "0x400",
    /// 		"parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    /// 		"proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    /// 		"timestamp": "0x5cd2b117",
    /// 		"transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    /// 		"version": "0x0"
    /// 	},
    /// 	"proposals": [],
    /// 	"transactions": [{
    /// 		"cell_deps": [],
    /// 		"header_deps": [],
    /// 		"inputs": [{
    /// 			"previous_output": {
    /// 				"index": "0xffffffff",
    /// 				"tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    /// 			},
    /// 			"since": "0x400"
    /// 		}],
    /// 		"outputs": [{
    /// 			"capacity": "0x18e64b61cf",
    /// 			"lock": {
    /// 				"code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    /// 				"hash_type": "data",
    /// 				"args": "0x"
    /// 			},
    /// 			"type": null
    /// 		}],
    /// 		"outputs_data": [
    /// 			"0x"
    /// 		],
    /// 		"version": "0x0",
    /// 		"witnesses": [
    /// 			"0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
    /// 		]
    /// 	}],
    /// 	"uncles": []
    ///     },
    ///     true
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///   "error": null
    /// }
    /// ```
    #[rpc(name = "process_block_without_verify")]
    fn process_block_without_verify(&self, data: Block, broadcast: bool) -> Result<Option<H256>>;

    /// Truncate chain to specified tip hash.
    ///
    /// ## Params
    ///
    /// * `target_tip_hash` - specified header hash
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "truncate",
    ///   "params": [
    ///     "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "truncate")]
    fn truncate(&self, target_tip_hash: H256) -> Result<()>;

    /// Generate block with block_assembler_config, process the block(with verification)
    ///
    /// and broadcast the block.
    ///
    /// ## Params
    ///
    /// * `block_assembler_script` - specified block assembler script
    ///
    /// * `block_assembler_message` - specified block assembler message
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "generate_block",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x60dd3fa0e81db3ee3ad41cf4ab956eae7e89eb71cd935101c26c4d0652db3029",
    ///   "error": null
    /// }
    /// ```
    #[rpc(name = "generate_block")]
    fn generate_block(&self) -> Result<H256>;

    /// Add transaction to tx-pool.
    ///
    /// ## Params
    ///
    /// * `transaction` - specified transaction to add
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    /// 	"id": 42,
    /// 	"jsonrpc": "2.0",
    /// 	"method": "notify_transaction",
    /// 	"params":
    ///     [
    ///          {
    /// 			"cell_deps": [{
    /// 				"dep_type": "code",
    /// 				"out_point": {
    /// 					"index": "0x0",
    /// 					"tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    /// 				}
    /// 			}],
    /// 			"header_deps": [
    /// 				"0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
    /// 			],
    /// 			"inputs": [{
    /// 				"previous_output": {
    /// 					"index": "0x0",
    /// 					"tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
    /// 				},
    /// 				"since": "0x0"
    /// 			}],
    /// 			"outputs": [{
    /// 				"capacity": "0x2540be400",
    /// 				"lock": {
    /// 					"code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    /// 					"hash_type": "data",
    /// 					"args": "0x"
    /// 				},
    /// 				"type": null
    /// 			}],
    /// 			"outputs_data": [
    /// 				"0x"
    /// 			],
    /// 			"version": "0x0",
    /// 			"witnesses": []
    /// 		}
    /// 	]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3",
    ///   "error": null
    /// }
    /// ```
    #[rpc(name = "notify_transaction")]
    fn notify_transaction(&self, transaction: Transaction) -> Result<H256>;

    /// Generate block with block template, attach calculated dao field to build new block,
    ///
    /// then process block and broadcast the block.
    ///
    /// ## Params
    ///
    /// * `block_template` - specified transaction to add
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "generate_block_with_template",
    ///   "params": [
    ///    {
    ///     "bytes_limit": "0x91c08",
    ///     "cellbase": {
    ///       "cycles": null,
    ///       "data": {
    ///         "cell_deps": [],
    ///         "header_deps": [],
    ///         "inputs": [
    ///           {
    ///             "previous_output": {
    ///               "index": "0xffffffff",
    ///               "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    ///             },
    ///             "since": "0x401"
    ///           }
    ///         ],
    ///        "outputs": [
    ///          {
    ///            "capacity": "0x18e64efc04",
    ///             "lock": {
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data",
    ///               "args": "0x"
    ///             },
    ///             "type": null
    ///           }
    ///         ],
    ///         "outputs_data": [
    ///           "0x"
    ///         ],
    ///         "version": "0x0",
    ///         "witnesses": [
    ///           "0x650000000c00000055000000490000001000000030000000310000001892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df20114000000b2e61ff569acf041b3c2c17724e2379c581eeac30c00000054455354206d657373616765"
    ///         ]
    ///       },
    ///       "hash": "0xbaf7e4db2fd002f19a597ca1a31dfe8cfe26ed8cebc91f52b75b16a7a5ec8bab"
    ///     },
    ///     "compact_target": "0x1e083126",
    ///     "current_time": "0x174c45e17a3",
    ///     "cycles_limit": "0xd09dc300",
    ///     "dao": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
    ///     "epoch": "0x7080019000001",
    ///     "extension": null,
    ///     "number": "0x401",
    ///     "parent_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "proposals": ["0xa0ef4eb5f4ceeb08a4c8"],
    ///     "transactions": [],
    ///     "uncles": [
    ///       {
    ///         "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
    ///         "header": {
    ///           "compact_target": "0x1e083126",
    ///           "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///           "epoch": "0x7080018000001",
    ///           "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///           "nonce": "0x0",
    ///           "number": "0x400",
    ///           "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///           "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///           "timestamp": "0x5cd2b118",
    ///           "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///           "version":"0x0"
    ///         },
    ///         "proposals": [],
    ///         "required": false
    ///       }
    ///     ],
    ///     "uncles_count_limit": "0x2",
    ///     "version": "0x0",
    ///     "work_id": "0x0"
    ///    }
    ///  ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x899541646ae412a99fdbefc081e1a782605a7815998a096af16e51d4df352c75",
    ///   "error": null
    /// }
    /// ```
    #[rpc(name = "generate_block_with_template")]
    fn generate_block_with_template(&self, block_template: BlockTemplate) -> Result<H256>;

    /// Return calculated dao field according to specified block template.
    ///
    /// ## Params
    ///
    /// * `block_template` - specified block template
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "calculate_dao_field",
    ///   "params": [
    ///    {
    ///     "bytes_limit": "0x91c08",
    ///     "cellbase": {
    ///       "cycles": null,
    ///       "data": {
    ///         "cell_deps": [],
    ///         "header_deps": [],
    ///         "inputs": [
    ///           {
    ///             "previous_output": {
    ///               "index": "0xffffffff",
    ///               "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    ///             },
    ///             "since": "0x401"
    ///           }
    ///         ],
    ///        "outputs": [
    ///          {
    ///            "capacity": "0x18e64efc04",
    ///             "lock": {
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data",
    ///               "args": "0x"
    ///             },
    ///             "type": null
    ///           }
    ///         ],
    ///         "outputs_data": [
    ///           "0x"
    ///         ],
    ///         "version": "0x0",
    ///         "witnesses": [
    ///           "0x650000000c00000055000000490000001000000030000000310000001892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df20114000000b2e61ff569acf041b3c2c17724e2379c581eeac30c00000054455354206d657373616765"
    ///         ]
    ///       },
    ///       "hash": "0xbaf7e4db2fd002f19a597ca1a31dfe8cfe26ed8cebc91f52b75b16a7a5ec8bab"
    ///     },
    ///     "compact_target": "0x1e083126",
    ///     "current_time": "0x174c45e17a3",
    ///     "cycles_limit": "0xd09dc300",
    ///     "dao": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
    ///     "epoch": "0x7080019000001",
    ///     "extension": null,
    ///     "number": "0x401",
    ///     "parent_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "proposals": ["0xa0ef4eb5f4ceeb08a4c8"],
    ///     "transactions": [],
    ///     "uncles": [
    ///       {
    ///         "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
    ///         "header": {
    ///           "compact_target": "0x1e083126",
    ///           "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///           "epoch": "0x7080018000001",
    ///           "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///           "nonce": "0x0",
    ///           "number": "0x400",
    ///           "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///           "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///           "timestamp": "0x5cd2b118",
    ///           "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///           "version":"0x0"
    ///         },
    ///         "proposals": [],
    ///         "required": false
    ///       }
    ///     ],
    ///     "uncles_count_limit": "0x2",
    ///     "version": "0x0",
    ///     "work_id": "0x0"
    ///    }
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
    ///   "error": null
    /// }
    /// ```
    #[rpc(name = "calculate_dao_field")]
    fn calculate_dao_field(&self, block_template: BlockTemplate) -> Result<Byte32>;
}

pub(crate) struct IntegrationTestRpcImpl {
    pub network_controller: NetworkController,
    pub shared: Shared,
    pub chain: ChainController,
}

impl IntegrationTestRpc for IntegrationTestRpcImpl {
    fn process_block_without_verify(&self, data: Block, broadcast: bool) -> Result<Option<H256>> {
        let block: packed::Block = data.into();
        let block: Arc<BlockView> = Arc::new(block.into_view());
        let ret = self
            .chain
            .internal_process_block(Arc::clone(&block), Switch::DISABLE_ALL);

        if broadcast {
            let content = packed::CompactBlock::build_from_block(&block, &HashSet::new());
            let message = packed::RelayMessage::new_builder().set(content).build();
            if let Err(err) = self
                .network_controller
                .quick_broadcast(SupportProtocols::RelayV2.protocol_id(), message.as_bytes())
            {
                error!("Broadcast new block failed: {:?}", err);
            }
        }
        if ret.is_ok() {
            Ok(Some(block.hash().unpack()))
        } else {
            error!("process_block_without_verify error: {:?}", ret);
            Ok(None)
        }
    }

    fn truncate(&self, target_tip_hash: H256) -> Result<()> {
        let header = {
            let snapshot = self.shared.snapshot();
            let header = snapshot
                .get_block_header(&target_tip_hash.pack())
                .ok_or_else(|| {
                    RPCError::custom(RPCError::Invalid, "block not found".to_string())
                })?;
            if !snapshot.is_main_chain(&header.hash()) {
                return Err(RPCError::custom(
                    RPCError::Invalid,
                    "block not on main chain".to_string(),
                ));
            }
            header
        };

        // Truncate the chain and database
        self.chain
            .truncate(header.hash())
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?;

        // Clear the tx_pool
        let new_snapshot = Arc::clone(&self.shared.snapshot());
        let tx_pool = self.shared.tx_pool_controller();
        tx_pool
            .clear_pool(new_snapshot)
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?;

        Ok(())
    }

    fn generate_block(&self) -> Result<H256> {
        let tx_pool = self.shared.tx_pool_controller();
        let block_template = tx_pool
            .get_block_template(None, None, None)
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?
            .map_err(|err| RPCError::custom(RPCError::CKBInternalError, err.to_string()))?;

        self.process_and_announce_block(block_template.into())
    }

    fn notify_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: packed::Transaction = tx.into();
        let tx: core::TransactionView = tx.into_view();
        let tx_pool = self.shared.tx_pool_controller();
        let tx_hash = tx.hash();
        if let Err(e) = tx_pool.notify_txs(vec![tx]) {
            error!("send notify_txs request error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        }
        Ok(tx_hash.unpack())
    }

    fn generate_block_with_template(&self, block_template: BlockTemplate) -> Result<H256> {
        let dao_field = self.calculate_dao_field(block_template.clone())?;

        let mut update_dao_template = block_template;
        update_dao_template.dao = dao_field;
        let block = update_dao_template.into();
        self.process_and_announce_block(block)
    }

    fn calculate_dao_field(&self, block_template: BlockTemplate) -> Result<Byte32> {
        let snapshot: &Snapshot = &self.shared.snapshot();
        let consensus = snapshot.consensus();
        let parent_header = snapshot
            .get_block_header(&block_template.parent_hash.pack())
            .expect("parent header should be stored");
        let mut seen_inputs = HashSet::new();

        let txs: Vec<_> = packed::Block::from(block_template)
            .transactions()
            .into_iter()
            .map(|tx| tx.into_view())
            .collect();

        let transactions_provider = TransactionsProvider::new(txs.as_slice().iter());
        let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, snapshot);
        let rtxs = txs
            .iter()
            .map(|tx| {
                resolve_transaction(
                    tx.clone(),
                    &mut seen_inputs,
                    &overlay_cell_provider,
                    snapshot,
                )
                .map_err(|err| {
                    error!(
                        "resolve transactions error when generating block \
                         with block template, error: {:?}",
                        err
                    );
                    RPCError::invalid_params(err.to_string())
                })
            })
            .collect::<Result<Vec<ResolvedTransaction>>>()?;

        Ok(DaoCalculator::new(consensus, &snapshot.as_data_provider())
            .dao_field(&rtxs, &parent_header)
            .expect("dao calculation should be OK")
            .into())
    }
}

impl IntegrationTestRpcImpl {
    fn process_and_announce_block(&self, block: packed::Block) -> Result<H256> {
        let block_view = Arc::new(block.into_view());
        let content = packed::CompactBlock::build_from_block(&block_view, &HashSet::new());
        let message = packed::RelayMessage::new_builder().set(content).build();

        // insert block to chain
        self.chain
            .process_block(Arc::clone(&block_view))
            .map_err(|err| RPCError::custom(RPCError::CKBInternalError, err.to_string()))?;

        // announce new block
        if let Err(err) = self
            .network_controller
            .quick_broadcast(SupportProtocols::RelayV2.protocol_id(), message.as_bytes())
        {
            error!("Broadcast new block failed: {:?}", err);
        }

        Ok(block_view.header().hash().unpack())
    }
}
