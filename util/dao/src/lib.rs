//! This crate provides implementation to calculate dao field.

use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::{extract_dao_data, pack_dao_data, DaoError};
use ckb_traits::{CellDataProvider, EpochProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::{CellMeta, ResolvedTransaction},
        Capacity, CapacityResult, HeaderView, ScriptHashType,
    },
    packed::{Byte32, CellOutput, Script, WitnessArgs},
    prelude::*,
};
use std::collections::HashSet;
use std::convert::TryFrom;

#[cfg(test)]
mod tests;

/// Dao field calculator
/// `DaoCalculator` is a facade to calculate the dao field.
pub struct DaoCalculator<'a, DL> {
    consensus: &'a Consensus,
    data_loader: &'a DL,
}

impl<'a, DL: CellDataProvider + EpochProvider + HeaderProvider> DaoCalculator<'a, DL> {
    /// Creates a new `DaoCalculator`.
    pub fn new(consensus: &'a Consensus, data_loader: &'a DL) -> Self {
        DaoCalculator {
            consensus,
            data_loader,
        }
    }

    /// Returns the primary block reward for `target` block.
    pub fn primary_block_reward(&self, target: &HeaderView) -> Result<Capacity, DaoError> {
        let target_epoch = self
            .data_loader
            .get_epoch_ext(target)
            .ok_or(DaoError::InvalidHeader)?;

        target_epoch
            .block_reward(target.number())
            .map_err(Into::into)
    }

    /// Returns the secondary block reward for `target` block.
    pub fn secondary_block_reward(&self, target: &HeaderView) -> Result<Capacity, DaoError> {
        if target.number() == 0 {
            return Ok(Capacity::zero());
        }

        let target_parent_hash = target.data().raw().parent_hash();
        let target_parent = self
            .data_loader
            .get_header(&target_parent_hash)
            .ok_or(DaoError::InvalidHeader)?;
        let target_epoch = self
            .data_loader
            .get_epoch_ext(target)
            .ok_or(DaoError::InvalidHeader)?;

        let target_g2 = target_epoch
            .secondary_block_issuance(target.number(), self.consensus.secondary_epoch_reward())?;
        let (_, target_parent_c, _, target_parent_u) = extract_dao_data(target_parent.dao());
        let reward128 = u128::from(target_g2.as_u64()) * u128::from(target_parent_u.as_u64())
            / u128::from(target_parent_c.as_u64());
        let reward = u64::try_from(reward128).map_err(|_| DaoError::Overflow)?;
        Ok(Capacity::shannons(reward))
    }

    /// Calculates the new dao field after packaging these transactions. It returns the dao field in [`Byte32`] format. Please see [`extract_dao_data`] if you intend to see the detailed content.
    ///
    /// [`Byte32`]: ../ckb_types/packed/struct.Byte32.html
    /// [`extract_dao_data`]: ../ckb_dao_utils/fn.extract_dao_data.html
    pub fn dao_field(
        &self,
        rtxs: &[ResolvedTransaction],
        parent: &HeaderView,
    ) -> Result<Byte32, DaoError> {
        // Freed occupied capacities from consumed inputs
        let freed_occupied_capacities =
            rtxs.iter().try_fold(Capacity::zero(), |capacities, rtx| {
                self.input_occupied_capacities(rtx)
                    .and_then(|c| capacities.safe_add(c))
            })?;
        let added_occupied_capacities = self.added_occupied_capacities(rtxs)?;
        let withdrawed_interests = self.withdrawed_interests(rtxs)?;

        let (parent_ar, parent_c, parent_s, parent_u) = extract_dao_data(parent.dao());

        // g contains both primary issuance and secondary issuance,
        // g2 is the secondary issuance for the block, which consists of
        // issuance for the miner, NervosDAO and treasury.
        // When calculating issuance in NervosDAO, we use the real
        // issuance for each block(which will only be issued on chain
        // after the finalization delay), not the capacities generated
        // in the cellbase of current block.
        let current_block_epoch = self
            .consensus
            .next_epoch_ext(&parent, self.data_loader)
            .ok_or(DaoError::InvalidHeader)?
            .epoch();
        let current_block_number = parent.number() + 1;
        let current_g2 = current_block_epoch.secondary_block_issuance(
            current_block_number,
            self.consensus.secondary_epoch_reward(),
        )?;
        let current_g = current_block_epoch
            .block_reward(current_block_number)
            .and_then(|c| c.safe_add(current_g2).map_err(Into::into))?;

        let miner_issuance128 = u128::from(current_g2.as_u64()) * u128::from(parent_u.as_u64())
            / u128::from(parent_c.as_u64());
        let miner_issuance =
            Capacity::shannons(u64::try_from(miner_issuance128).map_err(|_| DaoError::Overflow)?);
        let nervosdao_issuance = current_g2.safe_sub(miner_issuance)?;

        let current_c = parent_c.safe_add(current_g)?;
        let current_u = parent_u
            .safe_add(added_occupied_capacities)
            .and_then(|u| u.safe_sub(freed_occupied_capacities))?;
        let current_s = parent_s
            .safe_add(nervosdao_issuance)
            .and_then(|s| s.safe_sub(withdrawed_interests))?;

        let ar_increase128 =
            u128::from(parent_ar) * u128::from(current_g2.as_u64()) / u128::from(parent_c.as_u64());
        let ar_increase = u64::try_from(ar_increase128).map_err(|_| DaoError::Overflow)?;
        let current_ar = parent_ar
            .checked_add(ar_increase)
            .ok_or(DaoError::Overflow)?;

        Ok(pack_dao_data(current_ar, current_c, current_s, current_u))
    }

    /// Returns the total transactions fee of `rtx`.
    pub fn transaction_fee(&self, rtx: &ResolvedTransaction) -> Result<Capacity, DaoError> {
        let maximum_withdraw = self.transaction_maximum_withdraw(rtx)?;
        rtx.transaction
            .outputs_capacity()
            .and_then(|y| maximum_withdraw.safe_sub(y))
            .map_err(Into::into)
    }

    fn added_occupied_capacities(&self, rtxs: &[ResolvedTransaction]) -> CapacityResult<Capacity> {
        // Newly added occupied capacities from outputs
        let added_occupied_capacities =
            rtxs.iter().try_fold(Capacity::zero(), |capacities, rtx| {
                rtx.transaction
                    .outputs_with_data_iter()
                    .enumerate()
                    .try_fold(Capacity::zero(), |tx_capacities, (_, (output, data))| {
                        Capacity::bytes(data.len())
                            .and_then(|c| output.occupied_capacity(c))
                            .and_then(|c| tx_capacities.safe_add(c))
                    })
                    .and_then(|c| capacities.safe_add(c))
            })?;

        Ok(added_occupied_capacities)
    }

    fn input_occupied_capacities(&self, rtx: &ResolvedTransaction) -> CapacityResult<Capacity> {
        rtx.resolved_inputs
            .iter()
            .try_fold(Capacity::zero(), |capacities, cell_meta| {
                let current_capacity = modified_occupied_capacity(&cell_meta, &self.consensus);
                current_capacity.and_then(|c| capacities.safe_add(c))
            })
            .map_err(Into::into)
    }

    fn withdrawed_interests(&self, rtxs: &[ResolvedTransaction]) -> Result<Capacity, DaoError> {
        let maximum_withdraws = rtxs.iter().try_fold(Capacity::zero(), |capacities, rtx| {
            self.transaction_maximum_withdraw(rtx)
                .and_then(|c| capacities.safe_add(c).map_err(Into::into))
        })?;
        let input_capacities = rtxs.iter().try_fold(Capacity::zero(), |capacities, rtx| {
            let tx_input_capacities = rtx.resolved_inputs.iter().try_fold(
                Capacity::zero(),
                |tx_capacities, cell_meta| {
                    let output_capacity: Capacity = cell_meta.cell_output.capacity().unpack();
                    tx_capacities.safe_add(output_capacity)
                },
            )?;
            capacities.safe_add(tx_input_capacities)
        })?;
        maximum_withdraws
            .safe_sub(input_capacities)
            .map_err(Into::into)
    }

    fn transaction_maximum_withdraw(
        &self,
        rtx: &ResolvedTransaction,
    ) -> Result<Capacity, DaoError> {
        let header_deps: HashSet<Byte32> = rtx.transaction.header_deps_iter().collect();
        rtx.resolved_inputs.iter().enumerate().try_fold(
            Capacity::zero(),
            |capacities, (i, cell_meta)| {
                let capacity: Result<Capacity, DaoError> = {
                    let output = &cell_meta.cell_output;
                    let is_dao_type_script = |type_script: Script| {
                        Into::<u8>::into(type_script.hash_type())
                            == Into::<u8>::into(ScriptHashType::Type)
                            && type_script.code_hash()
                                == self.consensus.dao_type_hash().expect("No dao system cell")
                    };
                    let is_withdrawing_input =
                        |cell_meta: &CellMeta| match self.data_loader.load_cell_data(&cell_meta) {
                            Some(data) => data.len() == 8 && LittleEndian::read_u64(&data) > 0,
                            None => false,
                        };
                    if output
                        .type_()
                        .to_opt()
                        .map(is_dao_type_script)
                        .unwrap_or(false)
                        && is_withdrawing_input(&cell_meta)
                    {
                        let withdrawing_header_hash = cell_meta
                            .transaction_info
                            .as_ref()
                            .map(|info| &info.block_hash)
                            .filter(|hash| header_deps.contains(&hash))
                            .ok_or(DaoError::InvalidOutPoint)?;
                        let deposit_header_hash = rtx
                            .transaction
                            .witnesses()
                            .get(i)
                            .ok_or(DaoError::InvalidOutPoint)
                            .and_then(|witness_data| {
                                // dao contract stores header deps index as u64 in the input_type field of WitnessArgs
                                let witness = WitnessArgs::from_slice(&Unpack::<Bytes>::unpack(
                                    &witness_data,
                                ))
                                .map_err(|_| DaoError::InvalidDaoFormat)?;
                                let header_deps_index_data: Option<Bytes> = witness
                                    .input_type()
                                    .to_opt()
                                    .map(|witness| witness.unpack());
                                if header_deps_index_data.is_none()
                                    || header_deps_index_data.clone().map(|data| data.len())
                                        != Some(8)
                                {
                                    return Err(DaoError::InvalidDaoFormat);
                                }
                                Ok(LittleEndian::read_u64(&header_deps_index_data.unwrap()))
                            })
                            .and_then(|header_dep_index| {
                                rtx.transaction
                                    .header_deps()
                                    .get(header_dep_index as usize)
                                    .and_then(|hash| header_deps.get(&hash))
                                    .ok_or(DaoError::InvalidOutPoint)
                            })?;
                        self.calculate_maximum_withdraw(
                            &output,
                            Capacity::bytes(cell_meta.data_bytes as usize)?,
                            &deposit_header_hash,
                            &withdrawing_header_hash,
                        )
                    } else {
                        Ok(output.capacity().unpack())
                    }
                };
                capacity.and_then(|c| c.safe_add(capacities).map_err(Into::into))
            },
        )
    }

    /// Calculate maximum withdraw capacity of a deposited dao output
    pub fn calculate_maximum_withdraw(
        &self,
        output: &CellOutput,
        output_data_capacity: Capacity,
        deposit_header_hash: &Byte32,
        withdrawing_header_hash: &Byte32,
    ) -> Result<Capacity, DaoError> {
        let deposit_header = self
            .data_loader
            .get_header(deposit_header_hash)
            .ok_or(DaoError::InvalidHeader)?;
        let withdrawing_header = self
            .data_loader
            .get_header(withdrawing_header_hash)
            .ok_or(DaoError::InvalidHeader)?;
        if deposit_header.number() >= withdrawing_header.number() {
            return Err(DaoError::InvalidOutPoint);
        }

        let (deposit_ar, _, _, _) = extract_dao_data(deposit_header.dao());
        let (withdrawing_ar, _, _, _) = extract_dao_data(withdrawing_header.dao());

        let occupied_capacity = output.occupied_capacity(output_data_capacity)?;
        let output_capacity: Capacity = output.capacity().unpack();
        let counted_capacity = output_capacity.safe_sub(occupied_capacity)?;
        let withdraw_counted_capacity = u128::from(counted_capacity.as_u64())
            * u128::from(withdrawing_ar)
            / u128::from(deposit_ar);
        let withdraw_capacity =
            Capacity::shannons(withdraw_counted_capacity as u64).safe_add(occupied_capacity)?;

        Ok(withdraw_capacity)
    }
}

/// return special occupied capacity if cell is satoshi's gift
/// otherwise return cell occupied capacity
pub fn modified_occupied_capacity(
    cell_meta: &CellMeta,
    consensus: &Consensus,
) -> CapacityResult<Capacity> {
    if let Some(tx_info) = &cell_meta.transaction_info {
        if tx_info.is_genesis()
            && tx_info.is_cellbase()
            && cell_meta.cell_output.lock().args().raw_data() == consensus.satoshi_pubkey_hash.0[..]
        {
            return Unpack::<Capacity>::unpack(&cell_meta.cell_output.capacity())
                .safe_mul_ratio(consensus.satoshi_cell_occupied_ratio);
        }
    }
    cell_meta.occupied_capacity()
}
