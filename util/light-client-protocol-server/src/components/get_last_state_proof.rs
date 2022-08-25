use std::cmp::{max, min, Ordering};

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_shared::Snapshot;
use ckb_sync::ActiveChain;
use ckb_types::{core::BlockNumber, packed, prelude::*, U256};

use crate::{LightClientProtocol, Status, StatusCode};

pub(crate) struct GetLastStateProofProcess<'a> {
    message: packed::GetLastStateProofReader<'a>,
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
        if let Some(end_total_difficulty) = self.get_block_total_difficulty(end_block_number - 1) {
            if end_total_difficulty <= *min_total_difficulty {
                return None;
            }
        } else {
            return None;
        }
        let mut block_less_than_min = start_block_number;
        let mut block_greater_than_min = end_block_number - 1;
        loop {
            if block_greater_than_min == block_less_than_min + 1 {
                return Some(block_greater_than_min);
            }
            let next_number = (block_less_than_min + block_greater_than_min) / 2;
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
            } else {
                return None;
            }
        }
    }

    fn get_block_numbers_via_difficulties(
        &self,
        mut start_block_number: BlockNumber,
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
                if num > start_block_number {
                    start_block_number = num - 1;
                }
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
    ) -> Result<Vec<packed::VerifiableHeader>, String> {
        let active_chain = self.active_chain();
        let mut headers = Vec::new();

        for number in numbers {
            // Genesis block doesn't has chain root.
            if *number == 0 {
                continue;
            }
            if let Some(ancestor_header) = active_chain.get_ancestor(last_hash, *number) {
                let position = leaf_index_to_pos(*number);
                positions.push(position);

                let ancestor_block =
                    active_chain
                        .get_block(&ancestor_header.hash())
                        .ok_or_else(|| {
                            format!(
                                "failed to find block for header#{} (hash: {:#x})",
                                number,
                                ancestor_header.hash()
                            )
                        })?;
                let uncles_hash = ancestor_block.calc_uncles_hash();
                let extension = ancestor_block.extension();

                let parent_chain_root = {
                    let mmr = snapshot.chain_root_mmr(*number - 1);
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

                let header = packed::VerifiableHeader::new_builder()
                    .header(ancestor_header.data())
                    .uncles_hash(uncles_hash)
                    .extension(Pack::pack(&extension))
                    .parent_chain_root(parent_chain_root)
                    .build();

                headers.push(header);
            } else {
                let errmsg = format!("failed to find ancestor header ({})", number);
                return Err(errmsg);
            }
        }

        Ok(headers)
    }
}

impl<'a> GetLastStateProofProcess<'a> {
    pub(crate) fn new(
        message: packed::GetLastStateProofReader<'a>,
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

        let last_block_hash = self.message.last_hash().to_entity();
        let last_block = if let Some(block) = active_chain.get_block(&last_block_hash) {
            block
        } else {
            return self
                .protocol
                .reply_tip_state::<packed::SendLastStateProof>(self.peer, self.nc);
        };

        let snapshot = self.protocol.shared.shared().snapshot();

        let last_n_blocks: u64 = self.message.last_n_blocks().unpack();
        let start_block_hash = self.message.start_hash().to_entity();
        let start_block_number: BlockNumber = self.message.start_number().unpack();
        let mut difficulty_boundary: U256 = self.message.difficulty_boundary().unpack();
        let mut difficulties = self
            .message
            .difficulties()
            .iter()
            .map(|d| Unpack::<U256>::unpack(&d))
            .collect::<Vec<_>>();

        let last_block_number = last_block.number();

        let reorg_last_n_numbers = if start_block_number == 0
            || active_chain
                .get_ancestor(&last_block_hash, start_block_number)
                .map(|header| header.hash() == start_block_hash)
                .unwrap_or(false)
        {
            Vec::new()
        } else {
            // Genesis block doesn't has chain root.
            let min_block_number = max(
                1,
                start_block_number - min(start_block_number, last_n_blocks),
            );
            (min_block_number..start_block_number).collect()
        };

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

                if difficulty_boundary_block_number > 0 {
                    if let Some(total_difficulty) =
                        sampler.get_block_total_difficulty(difficulty_boundary_block_number - 1)
                    {
                        difficulty_boundary = total_difficulty;
                        difficulties = difficulties
                            .into_iter()
                            .take_while(|d| *d <= difficulty_boundary)
                            .collect();
                    } else {
                        let errmsg = format!(
                            "the total difficulty for block#{} is not found",
                            difficulty_boundary_block_number
                        );
                        return StatusCode::InternalError.with_context(errmsg);
                    };
                } else {
                    difficulties.clear();
                }
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

        let (positions, reorg_last_n_headers, sampled_headers, last_n_headers) = {
            let mut positions: Vec<u64> = Vec::new();
            let reorg_last_n_headers = match sampler.complete_headers(
                &snapshot,
                &mut positions,
                &last_block_hash,
                &reorg_last_n_numbers,
            ) {
                Ok(headers) => headers,
                Err(errmsg) => {
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
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
            (
                positions,
                reorg_last_n_headers,
                sampled_headers,
                last_n_headers,
            )
        };

        let proved_items = (
            reorg_last_n_headers.pack(),
            sampled_headers.pack(),
            last_n_headers.pack(),
        );

        self.protocol.reply_proof::<packed::SendLastStateProof>(
            self.peer,
            self.nc,
            &last_block,
            positions,
            proved_items,
        )
    }
}
