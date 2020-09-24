use crate::error::RPCError;
use ckb_jsonrpc_types::{
    BlockEconomicState, BlockNumber, BlockReward, BlockView, CellOutputWithOutPoint,
    CellWithStatus, EpochNumber, EpochView, HeaderView, MerkleProof as JsonMerkleProof, OutPoint,
    ResponseFormat, TransactionProof, TransactionWithStatus, Uint32,
};
use ckb_logger::error;
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::{
    core::{self, cell::CellProvider},
    packed::{self, Block, Header},
    prelude::*,
    utilities::{merkle_root, MerkleProof, CBMT},
    H256,
};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;

#[rpc(server)]
pub trait ChainRpc {
    #[rpc(name = "get_block")]
    fn get_block(
        &self,
        hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>>;

    #[rpc(name = "get_block_by_number")]
    fn get_block_by_number(
        &self,
        number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>>;

    #[rpc(name = "get_header")]
    fn get_header(
        &self,
        hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>>;

    #[rpc(name = "get_header_by_number")]
    fn get_header_by_number(
        &self,
        number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>>;

    #[rpc(name = "get_transaction")]
    fn get_transaction(&self, _hash: H256) -> Result<Option<TransactionWithStatus>>;

    #[rpc(name = "get_block_hash")]
    fn get_block_hash(&self, _number: BlockNumber) -> Result<Option<H256>>;

    #[rpc(name = "get_tip_header")]
    fn get_tip_header(
        &self,
        verbosity: Option<Uint32>,
    ) -> Result<ResponseFormat<HeaderView, Header>>;

    #[rpc(name = "deprecated.get_cells_by_lock_hash")]
    fn get_cells_by_lock_hash(
        &self,
        _lock_hash: H256,
        _from: BlockNumber,
        _to: BlockNumber,
    ) -> Result<Vec<CellOutputWithOutPoint>>;

    #[rpc(name = "get_live_cell")]
    fn get_live_cell(&self, _out_point: OutPoint, _with_data: bool) -> Result<CellWithStatus>;

    #[rpc(name = "get_tip_block_number")]
    fn get_tip_block_number(&self) -> Result<BlockNumber>;

    #[rpc(name = "get_current_epoch")]
    fn get_current_epoch(&self) -> Result<EpochView>;

    #[rpc(name = "get_epoch_by_number")]
    fn get_epoch_by_number(&self, number: EpochNumber) -> Result<Option<EpochView>>;

    #[rpc(name = "get_cellbase_output_capacity_details")]
    fn get_cellbase_output_capacity_details(&self, _hash: H256) -> Result<Option<BlockReward>>;

    #[rpc(name = "get_block_economic_state")]
    fn get_block_economic_state(&self, _hash: H256) -> Result<Option<BlockEconomicState>>;

    #[rpc(name = "get_transaction_proof")]
    fn get_transaction_proof(
        &self,
        tx_hashes: Vec<H256>,
        block_hash: Option<H256>,
    ) -> Result<TransactionProof>;

    #[rpc(name = "verify_transaction_proof")]
    fn verify_transaction_proof(&self, tx_proof: TransactionProof) -> Result<Vec<H256>>;
}

pub(crate) struct ChainRpcImpl {
    pub shared: Shared,
}

const DEFAULT_BLOCK_VERBOSITY_LEVEL: u32 = 2;
const DEFAULT_HEADER_VERBOSITY_LEVEL: u32 = 1;

impl ChainRpc for ChainRpcImpl {
    fn get_block(
        &self,
        hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = hash.pack();
        if !snapshot.is_main_chain(&block_hash) {
            return Ok(None);
        }

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_BLOCK_VERBOSITY_LEVEL);
        // TODO: verbosity level == 1, output block only contains tx_hash in json format
        if verbosity == 2 {
            Ok(snapshot
                .get_block(&block_hash)
                .map(|block| ResponseFormat::Json(block.into())))
        } else if verbosity == 0 {
            Ok(snapshot
                .get_packed_block(&block_hash)
                .map(ResponseFormat::Hex))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_block_by_number(
        &self,
        number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = match snapshot.get_block_hash(number.into()) {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_BLOCK_VERBOSITY_LEVEL);
        // TODO: verbosity level == 1, output block only contains tx_hash in json format
        let result = if verbosity == 2 {
            snapshot
                .get_block(&block_hash)
                .map(|block| Some(ResponseFormat::Json(block.into())))
        } else if verbosity == 0 {
            snapshot
                .get_packed_block(&block_hash)
                .map(|block| Some(ResponseFormat::Hex(block)))
        } else {
            return Err(RPCError::invalid_params("invalid verbosity level"));
        };

        result.ok_or_else(|| {
            let message = format!(
                "Chain Index says block #{} is {:#x}, but that block is not in the database",
                number, block_hash
            );
            error!("{}", message);
            RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
        })
    }

    fn get_header(
        &self,
        hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = hash.pack();
        if !snapshot.is_main_chain(&block_hash) {
            return Ok(None);
        }

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_HEADER_VERBOSITY_LEVEL);
        if verbosity == 1 {
            Ok(snapshot
                .get_block_header(&block_hash)
                .map(|header| ResponseFormat::Json(header.into())))
        } else if verbosity == 0 {
            Ok(snapshot
                .get_packed_block_header(&block_hash)
                .map(ResponseFormat::Hex))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_header_by_number(
        &self,
        number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = match snapshot.get_block_hash(number.into()) {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_HEADER_VERBOSITY_LEVEL);
        let result = if verbosity == 1 {
            snapshot
                .get_block_header(&block_hash)
                .map(|header| Some(ResponseFormat::Json(header.into())))
        } else if verbosity == 0 {
            snapshot
                .get_packed_block_header(&block_hash)
                .map(|header| Some(ResponseFormat::Hex(header)))
        } else {
            return Err(RPCError::invalid_params("invalid verbosity level"));
        };

        result.ok_or_else(|| {
            let message = format!(
                "Chain Index says block #{} is {:#x}, but that block is not in the database",
                number, block_hash
            );
            error!("{}", message);
            RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
        })
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<TransactionWithStatus>> {
        let hash = hash.pack();
        let id = packed::ProposalShortId::from_tx_hash(&hash);

        let tx = {
            let tx_pool = self.shared.tx_pool_controller();
            let fetch_tx_for_rpc = tx_pool.fetch_tx_for_rpc(id);
            if let Err(e) = fetch_tx_for_rpc {
                error!("send fetch_tx_for_rpc request error {}", e);
                return Err(RPCError::ckb_internal_error(e));
            };

            fetch_tx_for_rpc.unwrap().map(|(proposed, tx)| {
                if proposed {
                    TransactionWithStatus::with_proposed(tx)
                } else {
                    TransactionWithStatus::with_pending(tx)
                }
            })
        };

        Ok(tx.or_else(|| {
            self.shared
                .snapshot()
                .get_transaction(&hash)
                .map(|(tx, block_hash)| {
                    TransactionWithStatus::with_committed(tx, block_hash.unpack())
                })
        }))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Result<Option<H256>> {
        Ok(self
            .shared
            .snapshot()
            .get_block_hash(number.into())
            .map(|h| h.unpack()))
    }

    fn get_tip_header(
        &self,
        verbosity: Option<Uint32>,
    ) -> Result<ResponseFormat<HeaderView, Header>> {
        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_HEADER_VERBOSITY_LEVEL);
        if verbosity == 1 {
            Ok(ResponseFormat::Json(
                self.shared.snapshot().tip_header().clone().into(),
            ))
        } else if verbosity == 0 {
            Ok(ResponseFormat::Hex(
                self.shared.snapshot().tip_header().data(),
            ))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_current_epoch(&self) -> Result<EpochView> {
        Ok(EpochView::from_ext(
            self.shared.snapshot().epoch_ext().pack(),
        ))
    }

    fn get_epoch_by_number(&self, number: EpochNumber) -> Result<Option<EpochView>> {
        let snapshot = self.shared.snapshot();
        Ok(snapshot.get_epoch_index(number.into()).and_then(|hash| {
            snapshot
                .get_epoch_ext(&hash)
                .map(|ext| EpochView::from_ext(ext.pack()))
        }))
    }

    // TODO: we need to build a proper index instead of scanning every time
    fn get_cells_by_lock_hash(
        &self,
        _lock_hash: H256,
        _from: BlockNumber,
        _to: BlockNumber,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        Err(RPCError::custom(
            RPCError::Invalid,
            "get_cells_by_lock_hash have been deprecated, use [indexer] get_live_cells_by_lock_hash instead",
        ))
    }

    fn get_live_cell(&self, out_point: OutPoint, with_data: bool) -> Result<CellWithStatus> {
        let cell_status = self.shared.snapshot().cell(&out_point.into(), with_data);
        Ok(cell_status.into())
    }

    fn get_tip_block_number(&self) -> Result<BlockNumber> {
        Ok(self.shared.snapshot().tip_header().number().into())
    }

    fn get_cellbase_output_capacity_details(&self, hash: H256) -> Result<Option<BlockReward>> {
        let snapshot = self.shared.snapshot();

        if !snapshot.is_main_chain(&hash.pack()) {
            return Ok(None);
        }

        Ok(snapshot.get_block_header(&hash.pack()).and_then(|header| {
            snapshot
                .get_block_header(&header.data().raw().parent_hash())
                .and_then(|parent| {
                    if parent.number() < snapshot.consensus().finalization_delay_length() {
                        None
                    } else {
                        RewardCalculator::new(snapshot.consensus(), snapshot.as_ref())
                            .block_reward_to_finalize(&parent)
                            .map(|r| r.1.into())
                            .ok()
                    }
                })
        }))
    }

    fn get_block_economic_state(&self, hash: H256) -> Result<Option<BlockEconomicState>> {
        let snapshot = self.shared.snapshot();

        let block_number = if let Some(block_number) = snapshot.get_block_number(&hash.pack()) {
            block_number
        } else {
            return Ok(None);
        };

        let delay_length = snapshot.consensus().finalization_delay_length();
        let finalized_at_number = block_number + delay_length;
        if block_number == 0 || snapshot.tip_number() < finalized_at_number {
            return Ok(None);
        }

        let block_hash = hash.pack();
        let finalized_at = if let Some(block_hash) = snapshot.get_block_hash(finalized_at_number) {
            block_hash
        } else {
            return Ok(None);
        };

        let issuance = if let Some(issuance) = snapshot
            .get_block_epoch_index(&block_hash)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .and_then(|epoch_ext| {
                let primary = epoch_ext.block_reward(block_number).ok()?;
                let secondary = epoch_ext
                    .secondary_block_issuance(
                        block_number,
                        snapshot.consensus().secondary_epoch_reward(),
                    )
                    .ok()?;
                Some(core::BlockIssuance { primary, secondary })
            }) {
            issuance
        } else {
            return Ok(None);
        };

        let txs_fee = if let Some(txs_fee) =
            snapshot.get_block_ext(&block_hash).and_then(|block_ext| {
                block_ext
                    .txs_fees
                    .iter()
                    .try_fold(core::Capacity::zero(), |acc, tx_fee| acc.safe_add(*tx_fee))
                    .ok()
            }) {
            txs_fee
        } else {
            return Ok(None);
        };

        Ok(snapshot.get_block_header(&block_hash).and_then(|header| {
            RewardCalculator::new(snapshot.consensus(), snapshot.as_ref())
                .block_reward_for_target(&header)
                .ok()
                .map(|(_, block_reward)| core::BlockEconomicState {
                    issuance,
                    miner_reward: block_reward.into(),
                    txs_fee,
                    finalized_at,
                })
                .map(Into::into)
        }))
    }

    fn get_transaction_proof(
        &self,
        tx_hashes: Vec<H256>,
        block_hash: Option<H256>,
    ) -> Result<TransactionProof> {
        if tx_hashes.is_empty() {
            return Err(RPCError::invalid_params("Empty transaction hashes"));
        }
        let snapshot = self.shared.snapshot();

        let mut retrieved_block_hash = None;
        let mut tx_indices = HashSet::new();
        for tx_hash in tx_hashes {
            match snapshot.get_transaction_info(&tx_hash.pack()) {
                Some(tx_info) => {
                    if retrieved_block_hash.is_none() {
                        retrieved_block_hash = Some(tx_info.block_hash);
                    } else if Some(tx_info.block_hash) != retrieved_block_hash {
                        return Err(RPCError::invalid_params(
                            "Not all transactions found in retrieved block",
                        ));
                    }

                    if !tx_indices.insert(tx_info.index as u32) {
                        return Err(RPCError::invalid_params(format!(
                            "Duplicated tx_hash {:#x}",
                            tx_hash
                        )));
                    }
                }
                None => {
                    return Err(RPCError::invalid_params(format!(
                        "Transaction {:#x} not yet in block",
                        tx_hash
                    )));
                }
            }
        }

        let retrieved_block_hash = retrieved_block_hash.expect("checked len");
        if let Some(specified_block_hash) = block_hash {
            if !retrieved_block_hash.eq(&specified_block_hash.pack()) {
                return Err(RPCError::invalid_params(
                    "Not all transactions found in specified block",
                ));
            }
        }

        snapshot
            .get_block(&retrieved_block_hash)
            .ok_or_else(|| {
                let message = format!(
                    "Chain TransactionInfo says block {:#x} existing, but that block is not in the database",
                    retrieved_block_hash
                );
                error!("{}", message);
                RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
            })
            .map(|block| {
                let proof = CBMT::build_merkle_proof(
                    &block.transactions().iter().map(|tx| tx.hash()).collect::<Vec<_>>(),
                    &tx_indices.into_iter().collect::<Vec<_>>())
                .expect("build proof with verified inputs should be OK");
                TransactionProof {
                    block_hash: block.hash().unpack(),
                    witnesses_root: block.calc_witnesses_root().unpack(),
                    proof: JsonMerkleProof {
                        indices: proof.indices().iter().map(|index| (*index).into()).collect(),
                        lemmas: proof.lemmas().iter().map(|lemma| Unpack::<H256>::unpack(lemma)).collect(),
                    }
                }
            })
    }

    fn verify_transaction_proof(&self, tx_proof: TransactionProof) -> Result<Vec<H256>> {
        let snapshot = self.shared.snapshot();

        snapshot
            .get_block(&tx_proof.block_hash.pack())
            .ok_or_else(|| {
                RPCError::invalid_params(format!("Cannot find block {:#x}", tx_proof.block_hash))
            })
            .and_then(|block| {
                let witnesses_root = tx_proof.witnesses_root.pack();
                let merkle_proof = MerkleProof::new(
                    tx_proof
                        .proof
                        .indices
                        .into_iter()
                        .map(|index| index.value())
                        .collect(),
                    tx_proof
                        .proof
                        .lemmas
                        .into_iter()
                        .map(|lemma| lemma.pack())
                        .collect(),
                );

                CBMT::retrieve_leaves(&block.tx_hashes(), &merkle_proof)
                    .and_then(|tx_hashes| {
                        merkle_proof
                            .root(&tx_hashes)
                            .and_then(|raw_transactions_root| {
                                if block.transactions_root()
                                    == merkle_root(&[raw_transactions_root, witnesses_root])
                                {
                                    Some(tx_hashes.iter().map(|hash| hash.unpack()).collect())
                                } else {
                                    None
                                }
                            })
                    })
                    .ok_or_else(|| RPCError::invalid_params("Invalid transaction proof"))
            })
    }
}
