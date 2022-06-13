use std::cmp::Ordering;

use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, leaf_index_to_pos};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_shared::Snapshot;
use ckb_sync::ActiveChain;
use ckb_types::{
    core::BlockNumber, packed, prelude::*, utilities::merkle_mountain_range::ChainRootMMR, U256,
};

use crate::{prelude::*, LightClientProtocol, Status, StatusCode};

pub(crate) struct GetBlockProofProcess<'a> {
    message: packed::GetBlockProofReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

pub(crate) struct BlockSampler {
    active_chain: ActiveChain,
}

impl BlockSampler {
    fn new(active_chain: ActiveChain) -> Self {
        Self { active_chain }
    }

    fn active_chain(&self) -> &ActiveChain {
        &self.active_chain
    }

    fn get_block_total_difficulty(&self, number: BlockNumber) -> Option<U256> {
        self.active_chain()
            .get_block_hash(number)
            .and_then(|block_hash| self.active_chain().get_block_ext(&block_hash))
            .map(|block_ext| block_ext.total_difficulty)
    }

    fn get_first_block_total_difficulty_is_not_less_than(
        &self,
        start_block_number: BlockNumber,
        end_block_number: BlockNumber,
        min_total_difficulty: &U256,
    ) -> Option<BlockNumber> {
        if let Some(start_total_difficulty) = self.get_block_total_difficulty(start_block_number) {
            if start_total_difficulty >= *min_total_difficulty {
                return Some(start_block_number);
            }
        } else {
            return None;
        }
        if let Some(end_total_difficulty) = self.get_block_total_difficulty(end_block_number) {
            if end_total_difficulty <= *min_total_difficulty {
                return None;
            }
        } else {
            return None;
        }
        let mut block_less_than_min = start_block_number;
        let mut block_greater_than_min = end_block_number;
        let mut next_number = (block_less_than_min + block_greater_than_min) / 2;
        loop {
            if block_greater_than_min == block_less_than_min + 1 {
                return Some(block_greater_than_min);
            }
            if let Some(total_difficulty) = self.get_block_total_difficulty(next_number) {
                match total_difficulty.cmp(min_total_difficulty) {
                    Ordering::Equal => {
                        return Some(next_number);
                    }
                    Ordering::Less => {
                        block_less_than_min = next_number;
                    }
                    Ordering::Greater => {
                        block_greater_than_min = next_number;
                    }
                }
                next_number = (block_less_than_min + block_greater_than_min) / 2;
            } else {
                return None;
            }
        }
    }

    fn get_block_numbers_via_difficulties(
        &self,
        start_block_number: BlockNumber,
        end_block_number: BlockNumber,
        difficulties: &[U256],
    ) -> Result<Vec<BlockNumber>, String> {
        let mut numbers = Vec::new();
        for difficulty in difficulties {
            if let Some(num) = self.get_first_block_total_difficulty_is_not_less_than(
                start_block_number,
                end_block_number,
                difficulty,
            ) {
                numbers.push(num);
            } else {
                let errmsg = format!(
                    "the difficulty ({:#x}) is not in the block range [{}, {}]",
                    difficulty, start_block_number, end_block_number,
                );
                return Err(errmsg);
            }
        }
        numbers.dedup();
        Ok(numbers)
    }

    fn complete_headers(
        &self,
        snapshot: &Snapshot,
        positions: &mut Vec<u64>,
        last_hash: &packed::Byte32,
        numbers: &[BlockNumber],
    ) -> Result<Vec<packed::HeaderWithChainRoot>, String> {
        let active_chain = self.active_chain();
        let mut headers_with_chain_root = Vec::new();

        for number in numbers {
            // Genesis block doesn't has chain root.
            if *number == 0 {
                continue;
            }
            if let Some(ancestor_header) = active_chain.get_ancestor(last_hash, *number) {
                let position = leaf_index_to_pos(*number);
                positions.push(position);

                let uncles_hash = match active_chain.get_block(&ancestor_header.hash()) {
                    Some(ancestor_block) => ancestor_block.calc_uncles_hash(),
                    None => {
                        let errmsg = format!(
                            "failed to find block for header#{} (hash: {:#x})",
                            number,
                            ancestor_header.hash()
                        );
                        return Err(errmsg);
                    }
                };

                let chain_root = {
                    let mmr_size = leaf_index_to_mmr_size(*number - 1);
                    let mmr = ChainRootMMR::new(mmr_size, snapshot);
                    match mmr.get_root() {
                        Ok(root) => root,
                        Err(err) => {
                            let errmsg = format!(
                                "failed to generate a root for block#{} since {:?}",
                                number, err
                            );
                            return Err(errmsg);
                        }
                    }
                };

                let header_with_chain_root = packed::HeaderWithChainRoot::new_builder()
                    .header(ancestor_header.data())
                    .uncles_hash(uncles_hash)
                    .chain_root(chain_root)
                    .build();

                headers_with_chain_root.push(header_with_chain_root);
            } else {
                let errmsg = format!("failed to find ancestor header ({})", number);
                return Err(errmsg);
            }
        }

        Ok(headers_with_chain_root)
    }
}

impl<'a> GetBlockProofProcess<'a> {
    pub(crate) fn new(
        message: packed::GetBlockProofReader<'a>,
        protocol: &'a LightClientProtocol,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        Self {
            message,
            protocol,
            peer,
            nc,
        }
    }

    pub(crate) fn execute(self) -> Status {
        let active_chain = self.protocol.shared.active_chain();
        let snapshot = self.protocol.shared.shared().snapshot();

        let last_block_hash = self.message.last_hash().to_entity();
        let start_block_number: BlockNumber = self.message.start_number().unpack();
        let last_n_blocks: BlockNumber = self.message.last_n_blocks().unpack();
        let mut difficulty_boundary: U256 = self.message.difficulty_boundary().unpack();
        let mut difficulties = self
            .message
            .difficulties()
            .iter()
            .map(|d| Unpack::<U256>::unpack(&d))
            .collect::<Vec<_>>();

        let last_block_header =
            if let Some(block_header) = active_chain.get_block_header(&last_block_hash) {
                block_header
            } else {
                let errmsg = format!(
                    "the last block ({:#x}) sent from the client is not existed",
                    last_block_hash
                );
                return StatusCode::InvalidLastBlock.with_context(errmsg);
            };
        let last_block_number = last_block_header.number();

        let sampler = BlockSampler::new(active_chain);

        // Check the request data.
        {
            // The difficulties should be sorted.
            if difficulties.windows(2).any(|d| d[0] >= d[1]) {
                let errmsg = "the difficulties should be monotonically increasing";
                return StatusCode::InvalidRequest.with_context(errmsg);
            }
            // The maximum difficulty should be less than the difficulty boundary.
            if difficulties
                .last()
                .map(|d| *d >= difficulty_boundary)
                .unwrap_or(false)
            {
                let errmsg = "the difficulty boundary should be greater than all difficulties";
                return StatusCode::InvalidRequest.with_context(errmsg);
            }
            // The first difficulty should be greater than the total difficulty before the start block.
            if let Some(start_difficulty) = difficulties.get(0) {
                if start_block_number > 0 {
                    let previous_block_number = start_block_number - 1;
                    if let Some(total_difficulty) =
                        sampler.get_block_total_difficulty(previous_block_number)
                    {
                        if total_difficulty >= *start_difficulty {
                            let errmsg = format!(
                                "the start difficulty is {:#x} too less than \
                                the previous block #{} of the start block",
                                start_difficulty, previous_block_number
                            );
                            return StatusCode::InvalidRequest.with_context(errmsg);
                        }
                    } else {
                        let errmsg = format!(
                            "the total difficulty for block#{} is not found",
                            previous_block_number
                        );
                        return StatusCode::InternalError.with_context(errmsg);
                    };
                }
            }
            // The last block should be in main chain.
            if !sampler.active_chain().is_main_chain(&last_block_hash) {
                let errmsg = format!(
                    "the last block ({:#x}) sent from the client is not in the main chain",
                    last_block_hash
                );
                return StatusCode::InvalidLastBlock.with_context(errmsg);
            }
        }

        let (sampled_numbers, last_n_numbers) =
            if last_block_number - start_block_number <= last_n_blocks {
                // There is not enough blocks, so we take all of them; so there is no sampled blocks.
                let sampled_numbers = Vec::new();
                let last_n_numbers = (start_block_number..last_block_number)
                    .into_iter()
                    .collect::<Vec<_>>();
                (sampled_numbers, last_n_numbers)
            } else {
                let mut difficulty_boundary_block_number = if let Some(block_number) = sampler
                    .get_first_block_total_difficulty_is_not_less_than(
                        start_block_number,
                        last_block_number,
                        &difficulty_boundary,
                    ) {
                    block_number
                } else {
                    let errmsg = format!(
                        "the difficulty boundary ({:#x}) is not in the block range [{}, {})",
                        difficulty_boundary, start_block_number, last_block_number,
                    );
                    return StatusCode::InvaildDifficultyBoundary.with_context(errmsg);
                };

                if last_block_number - difficulty_boundary_block_number < last_n_blocks {
                    // There is not enough blocks after the difficulty boundary, so we take more.
                    difficulty_boundary_block_number = last_block_number - last_n_blocks;
                }

                if let Some(total_difficulty) =
                    sampler.get_block_total_difficulty(difficulty_boundary_block_number)
                {
                    difficulty_boundary = total_difficulty;
                    difficulties = difficulties
                        .into_iter()
                        .take_while(|d| *d < difficulty_boundary)
                        .collect();
                } else {
                    let errmsg = format!(
                        "the total difficulty for block#{} is not found",
                        difficulty_boundary_block_number
                    );
                    return StatusCode::InternalError.with_context(errmsg);
                };
                let sampled_numbers = match sampler.get_block_numbers_via_difficulties(
                    start_block_number,
                    difficulty_boundary_block_number,
                    &difficulties,
                ) {
                    Ok(numbers) => numbers,
                    Err(errmsg) => {
                        return StatusCode::InternalError.with_context(errmsg);
                    }
                };
                let last_n_numbers = (difficulty_boundary_block_number..last_block_number)
                    .into_iter()
                    .collect::<Vec<_>>();

                (sampled_numbers, last_n_numbers)
            };

        let (positions, sampled_headers, last_n_headers) = {
            let mut positions: Vec<u64> = Vec::new();
            let sampled_headers = match sampler.complete_headers(
                &snapshot,
                &mut positions,
                &last_block_hash,
                &sampled_numbers,
            ) {
                Ok(headers) => headers,
                Err(errmsg) => {
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            let last_n_headers = match sampler.complete_headers(
                &snapshot,
                &mut positions,
                &last_block_hash,
                &last_n_numbers,
            ) {
                Ok(headers) => headers,
                Err(errmsg) => {
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            (positions, sampled_headers, last_n_headers)
        };

        let (root, proof) = {
            let mmr_size = leaf_index_to_mmr_size(last_block_number - 1);
            let snapshot_ref: &Snapshot = &snapshot;
            let mmr = ChainRootMMR::new(mmr_size, snapshot_ref);
            let root = match mmr.get_root() {
                Ok(root) => root,
                Err(err) => {
                    let errmsg = format!("failed to generate a root since {:?}", err);
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            let proof = match mmr.gen_proof(positions) {
                Ok(proof) => proof,
                Err(err) => {
                    let errmsg = format!("failed to generate a proof since {:?}", err);
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            (root, proof)
        };

        let content = packed::SendBlockProof::new_builder()
            .root(root)
            .proof(proof.pack())
            .sampled_headers(sampled_headers.pack())
            .last_n_headers(last_n_headers.pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        self.nc.reply(self.peer, &message)
    }
}
