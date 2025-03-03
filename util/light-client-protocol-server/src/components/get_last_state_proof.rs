use std::cmp::{Ordering, min};

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_shared::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{U256, core::BlockNumber, packed, prelude::*};

use crate::{LightClientProtocol, Status, StatusCode, constant};

pub(crate) struct GetLastStateProofProcess<'a> {
    message: packed::GetLastStateProofReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

pub(crate) trait FindBlocksViaDifficulties {
    fn get_block_total_difficulty(&self, number: BlockNumber) -> Option<U256>;

    fn get_first_block_total_difficulty_is_not_less_than(
        &self,
        start_block_number: BlockNumber,
        end_block_number: BlockNumber,
        min_total_difficulty: &U256,
    ) -> Option<(BlockNumber, U256)> {
        if let Some(start_total_difficulty) = self.get_block_total_difficulty(start_block_number) {
            if start_total_difficulty >= *min_total_difficulty {
                return Some((start_block_number, start_total_difficulty));
            }
        } else {
            return None;
        }
        let mut end_total_difficulty = if let Some(end_total_difficulty) =
            self.get_block_total_difficulty(end_block_number - 1)
        {
            if end_total_difficulty < *min_total_difficulty {
                return None;
            }
            end_total_difficulty
        } else {
            return None;
        };
        let mut block_less_than_min = start_block_number;
        let mut block_greater_than_min = end_block_number - 1;
        loop {
            if block_greater_than_min == block_less_than_min + 1 {
                return Some((block_greater_than_min, end_total_difficulty));
            }
            let next_number = (block_less_than_min + block_greater_than_min) / 2;
            if let Some(total_difficulty) = self.get_block_total_difficulty(next_number) {
                match total_difficulty.cmp(min_total_difficulty) {
                    Ordering::Equal => {
                        return Some((next_number, total_difficulty));
                    }
                    Ordering::Less => {
                        block_less_than_min = next_number;
                    }
                    Ordering::Greater => {
                        block_greater_than_min = next_number;
                        end_total_difficulty = total_difficulty;
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
        let mut current_difficulty = U256::zero();
        for difficulty in difficulties {
            if current_difficulty >= *difficulty {
                continue;
            }
            if let Some((num, diff)) = self.get_first_block_total_difficulty_is_not_less_than(
                start_block_number,
                end_block_number,
                difficulty,
            ) {
                if num > start_block_number {
                    start_block_number = num - 1;
                }
                numbers.push(num);
                current_difficulty = diff;
            } else {
                let errmsg = format!(
                    "the difficulty ({difficulty:#x}) is not in the block range [{start_block_number}, {end_block_number})"
                );
                return Err(errmsg);
            }
        }
        Ok(numbers)
    }
}

pub(crate) struct BlockSampler<'a> {
    snapshot: &'a Snapshot,
}

impl<'a> FindBlocksViaDifficulties for BlockSampler<'a> {
    fn get_block_total_difficulty(&self, number: BlockNumber) -> Option<U256> {
        self.snapshot
            .get_block_hash(number)
            .and_then(|block_hash| self.snapshot.get_block_ext(&block_hash))
            .map(|block_ext| block_ext.total_difficulty)
    }
}

impl<'a> BlockSampler<'a> {
    fn new(snapshot: &'a Snapshot) -> Self {
        Self { snapshot }
    }

    fn complete_headers(
        &self,
        positions: &mut Vec<u64>,
        last_hash: &packed::Byte32,
        numbers: &[BlockNumber],
    ) -> Result<Vec<packed::VerifiableHeader>, String> {
        let mut headers = Vec::new();

        for number in numbers {
            if let Some(ancestor_header) = self.snapshot.get_ancestor(last_hash, *number) {
                let position = leaf_index_to_pos(*number);
                positions.push(position);

                let ancestor_block = self
                    .snapshot
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

                let parent_chain_root = if *number == 0 {
                    Default::default()
                } else {
                    let mmr = self.snapshot.chain_root_mmr(*number - 1);
                    match mmr.get_root() {
                        Ok(root) => root,
                        Err(err) => {
                            let errmsg = format!(
                                "failed to generate a root for block#{number} since {err:?}"
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
                let errmsg = format!("failed to find ancestor header ({number})");
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
        let last_n_blocks: u64 = self.message.last_n_blocks().unpack();

        if self.message.difficulties().len() + (last_n_blocks as usize) * 2
            > constant::GET_LAST_STATE_PROOF_LIMIT
        {
            return StatusCode::MalformedProtocolMessage.with_context("too many samples");
        }

        let snapshot = self.protocol.shared.snapshot();

        let last_block_hash = self.message.last_hash().to_entity();
        if !snapshot.is_main_chain(&last_block_hash) {
            return self
                .protocol
                .reply_tip_state::<packed::SendLastStateProof>(self.peer, self.nc);
        }
        let last_block = snapshot
            .get_block(&last_block_hash)
            .expect("block should be in store");

        let start_block_hash = self.message.start_hash().to_entity();
        let start_block_number: BlockNumber = self.message.start_number().unpack();
        let difficulty_boundary: U256 = self.message.difficulty_boundary().unpack();
        let mut difficulties = self
            .message
            .difficulties()
            .iter()
            .map(|d| Unpack::<U256>::unpack(&d))
            .collect::<Vec<_>>();

        let last_block_number = last_block.number();

        let reorg_last_n_numbers = if start_block_number == 0
            || snapshot
                .get_ancestor(&last_block_hash, start_block_number)
                .map(|header| header.hash() == start_block_hash)
                .unwrap_or(false)
        {
            Vec::new()
        } else {
            let min_block_number = start_block_number - min(start_block_number, last_n_blocks);
            (min_block_number..start_block_number).collect()
        };

        let sampler = BlockSampler::new(&snapshot);

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
            if let Some(start_difficulty) = difficulties.first() {
                if start_block_number > 0 {
                    let previous_block_number = start_block_number - 1;
                    if let Some(total_difficulty) =
                        sampler.get_block_total_difficulty(previous_block_number)
                    {
                        if total_difficulty >= *start_difficulty {
                            let errmsg = format!(
                                "the start difficulty is {start_difficulty:#x} too less than \
                                the previous block #{previous_block_number} of the start block"
                            );
                            return StatusCode::InvalidRequest.with_context(errmsg);
                        }
                    } else {
                        let errmsg = format!(
                            "the total difficulty for block#{previous_block_number} is not found"
                        );
                        return StatusCode::InternalError.with_context(errmsg);
                    };
                }
            }
        }

        let (sampled_numbers, last_n_numbers) = if last_block_number - start_block_number
            <= last_n_blocks
        {
            // There is not enough blocks, so we take all of them; so there is no sampled blocks.
            let sampled_numbers = Vec::new();
            let last_n_numbers = (start_block_number..last_block_number).collect::<Vec<_>>();
            (sampled_numbers, last_n_numbers)
        } else {
            let mut difficulty_boundary_block_number = if let Some((num, _)) = sampler
                .get_first_block_total_difficulty_is_not_less_than(
                    start_block_number,
                    last_block_number,
                    &difficulty_boundary,
                ) {
                num
            } else {
                let errmsg = format!(
                    "the difficulty boundary ({difficulty_boundary:#x}) is not in the block range [{start_block_number}, {last_block_number})"
                );
                return StatusCode::InvaildDifficultyBoundary.with_context(errmsg);
            };

            if last_block_number - difficulty_boundary_block_number < last_n_blocks {
                // There is not enough blocks after the difficulty boundary, so we take more.
                difficulty_boundary_block_number = last_block_number - last_n_blocks;
            }

            let last_n_numbers =
                (difficulty_boundary_block_number..last_block_number).collect::<Vec<_>>();

            if difficulty_boundary_block_number > 0 {
                if let Some(total_difficulty) =
                    sampler.get_block_total_difficulty(difficulty_boundary_block_number - 1)
                {
                    difficulties = difficulties
                        .into_iter()
                        .take_while(|d| *d <= total_difficulty)
                        .collect();
                } else {
                    let errmsg = format!(
                        "the total difficulty for block#{difficulty_boundary_block_number} is not found"
                    );
                    return StatusCode::InternalError.with_context(errmsg);
                };
                match sampler.get_block_numbers_via_difficulties(
                    start_block_number,
                    difficulty_boundary_block_number,
                    &difficulties,
                ) {
                    Ok(sampled_numbers) => (sampled_numbers, last_n_numbers),
                    Err(errmsg) => {
                        return StatusCode::InternalError.with_context(errmsg);
                    }
                }
            } else {
                (Vec::new(), last_n_numbers)
            }
        };

        let block_numbers = reorg_last_n_numbers
            .into_iter()
            .chain(sampled_numbers)
            .chain(last_n_numbers)
            .collect::<Vec<_>>();

        let (positions, headers) = {
            let mut positions: Vec<u64> = Vec::new();
            let headers =
                match sampler.complete_headers(&mut positions, &last_block_hash, &block_numbers) {
                    Ok(headers) => headers,
                    Err(errmsg) => {
                        return StatusCode::InternalError.with_context(errmsg);
                    }
                };
            (positions, headers)
        };

        let proved_items = headers.pack();

        self.protocol.reply_proof::<packed::SendLastStateProof>(
            self.peer,
            self.nc,
            &last_block,
            positions,
            proved_items,
            (),
        )
    }
}
