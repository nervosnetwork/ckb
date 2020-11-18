//! TODO(doc): @keroro520
use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::{extract_dao_data, pack_dao_data, DaoError};
use ckb_error::Error;
use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainStore};
use ckb_traits::CellDataProvider;
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::{CellMeta, ResolvedTransaction},
        Capacity, CapacityResult, HeaderView, ScriptHashType,
    },
    packed::{Byte32, CellOutput, OutPoint, Script, WitnessArgs},
    prelude::*,
};
use std::collections::HashSet;
use std::convert::TryFrom;

/// TODO(doc): @keroro520
pub struct DaoCalculator<'a, CS, DL> {
    /// TODO(doc): @keroro520
    pub consensus: &'a Consensus,
    /// TODO(doc): @keroro520
    pub store: &'a CS,
    /// TODO(doc): @keroro520
    pub data_loader: DL,
}

impl<'a, CS: ChainStore<'a>> DaoCalculator<'a, CS, DataLoaderWrapper<'a, CS>> {
    /// TODO(doc): @keroro520
    pub fn new(consensus: &'a Consensus, store: &'a CS) -> Self {
        let data_loader = DataLoaderWrapper::new(store);
        DaoCalculator {
            consensus,
            store,
            data_loader,
        }
    }

    /// TODO(doc): @keroro520
    pub fn primary_block_reward(&self, target: &HeaderView) -> Result<Capacity, Error> {
        let target_epoch = self
            .store
            .get_block_epoch_index(&target.hash())
            .and_then(|index| self.store.get_epoch_ext(&index))
            .ok_or(DaoError::InvalidHeader)?;

        target_epoch.block_reward(target.number())
    }

    /// TODO(doc): @keroro520
    pub fn secondary_block_reward(&self, target: &HeaderView) -> Result<Capacity, Error> {
        if target.number() == 0 {
            return Ok(Capacity::zero());
        }

        let target_parent_hash = target.data().raw().parent_hash();
        let target_parent = self
            .store
            .get_block_header(&target_parent_hash)
            .ok_or(DaoError::InvalidHeader)?;
        let target_epoch = self
            .store
            .get_block_epoch_index(&target.hash())
            .and_then(|index| self.store.get_epoch_ext(&index))
            .ok_or(DaoError::InvalidHeader)?;

        let target_g2 = target_epoch
            .secondary_block_issuance(target.number(), self.consensus.secondary_epoch_reward())?;
        let (_, target_parent_c, _, target_parent_u) = extract_dao_data(target_parent.dao())?;
        let reward128 = u128::from(target_g2.as_u64()) * u128::from(target_parent_u.as_u64())
            / u128::from(target_parent_c.as_u64());
        let reward = u64::try_from(reward128).map_err(|_| DaoError::Overflow)?;
        Ok(Capacity::shannons(reward))
    }

    /// TODO(doc): @keroro520
    // Used for testing only.
    //
    // Notice unlike primary_block_reward and secondary_epoch_reward above,
    // this starts calculating from parent, not target header.
    pub fn base_block_reward(&self, parent: &HeaderView) -> Result<Capacity, Error> {
        let target_number = self
            .consensus
            .finalize_target(parent.number() + 1)
            .ok_or(DaoError::InvalidHeader)?;
        let target = self
            .store
            .get_block_hash(target_number)
            .and_then(|hash| self.store.get_block_header(&hash))
            .ok_or(DaoError::InvalidHeader)?;

        let primary_block_reward = self.primary_block_reward(&target)?;
        let secondary_block_reward = self.secondary_block_reward(&target)?;

        Ok(primary_block_reward.safe_add(secondary_block_reward)?)
    }

    /// TODO(doc): @keroro520
    pub fn dao_field(
        &self,
        rtxs: &[ResolvedTransaction],
        parent: &HeaderView,
    ) -> Result<Byte32, Error> {
        // Freed occupied capacities from consumed inputs
        let freed_occupied_capacities =
            rtxs.iter().try_fold(Capacity::zero(), |capacities, rtx| {
                self.input_occupied_capacities(rtx)
                    .and_then(|c| capacities.safe_add(c).map_err(Into::into))
            })?;
        let added_occupied_capacities = self.added_occupied_capacities(rtxs)?;
        let withdrawed_interests = self.withdrawed_interests(rtxs)?;

        let (parent_ar, parent_c, parent_s, parent_u) = extract_dao_data(parent.dao())?;

        // g contains both primary issuance and secondary issuance,
        // g2 is the secondary issuance for the block, which consists of
        // issuance for the miner, NervosDAO and treasury.
        // When calculating issuance in NervosDAO, we use the real
        // issuance for each block(which will only be issued on chain
        // after the finalization delay), not the capacities generated
        // in the cellbase of current block.
        let parent_block_epoch = self
            .store
            .get_block_epoch_index(&parent.hash())
            .and_then(|index| self.store.get_epoch_ext(&index))
            .ok_or(DaoError::InvalidHeader)?;
        let current_block_epoch = self
            .store
            .next_epoch_ext(&self.consensus, &parent_block_epoch, &parent)
            .unwrap_or(parent_block_epoch);
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

    /// TODO(doc): @keroro520
    pub fn maximum_withdraw(
        &self,
        out_point: &OutPoint,
        withdrawing_header_hash: &Byte32,
    ) -> Result<Capacity, Error> {
        let (tx, block_hash) = self
            .store
            .get_transaction(&out_point.tx_hash())
            .ok_or(DaoError::InvalidOutPoint)?;
        let output = tx
            .outputs()
            .get(out_point.index().unpack())
            .ok_or(DaoError::InvalidOutPoint)?;
        let output_data = tx
            .outputs_data()
            .get(out_point.index().unpack())
            .ok_or(DaoError::InvalidOutPoint)?;
        self.calculate_maximum_withdraw(
            &output,
            Capacity::bytes(output_data.len())?,
            &block_hash,
            withdrawing_header_hash,
        )
    }

    /// TODO(doc): @keroro520
    pub fn transaction_fee(&self, rtx: &ResolvedTransaction) -> Result<Capacity, Error> {
        let maximum_withdraw = self.transaction_maximum_withdraw(rtx)?;
        rtx.transaction
            .outputs_capacity()
            .and_then(|y| maximum_withdraw.safe_sub(y))
            .map_err(Into::into)
    }

    fn added_occupied_capacities(&self, rtxs: &[ResolvedTransaction]) -> Result<Capacity, Error> {
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

    fn input_occupied_capacities(&self, rtx: &ResolvedTransaction) -> Result<Capacity, Error> {
        rtx.resolved_inputs
            .iter()
            .try_fold(Capacity::zero(), |capacities, cell_meta| {
                let current_capacity = modified_occupied_capacity(&cell_meta, &self.consensus);
                current_capacity.and_then(|c| capacities.safe_add(c))
            })
            .map_err(Into::into)
    }

    fn withdrawed_interests(&self, rtxs: &[ResolvedTransaction]) -> Result<Capacity, Error> {
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

    fn transaction_maximum_withdraw(&self, rtx: &ResolvedTransaction) -> Result<Capacity, Error> {
        let header_deps: HashSet<Byte32> = rtx.transaction.header_deps_iter().collect();
        rtx.resolved_inputs.iter().enumerate().try_fold(
            Capacity::zero(),
            |capacities, (i, cell_meta)| {
                let capacity: Result<Capacity, Error> = {
                    let output = &cell_meta.cell_output;
                    let is_dao_type_script = |type_script: Script| {
                        Into::<u8>::into(type_script.hash_type())
                            == Into::<u8>::into(ScriptHashType::Type)
                            && type_script.code_hash()
                                == self.consensus.dao_type_hash().expect("No dao system cell")
                    };
                    let is_withdrawing_input =
                        |cell_meta: &CellMeta| match self.data_loader.load_cell_data(&cell_meta) {
                            Some((data, _)) => data.len() == 8 && LittleEndian::read_u64(&data) > 0,
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

    fn calculate_maximum_withdraw(
        &self,
        output: &CellOutput,
        output_data_capacity: Capacity,
        deposit_header_hash: &Byte32,
        withdrawing_header_hash: &Byte32,
    ) -> Result<Capacity, Error> {
        let deposit_header = self
            .store
            .get_block_header(deposit_header_hash)
            .ok_or(DaoError::InvalidHeader)?;
        let withdrawing_header = self
            .store
            .get_block_header(withdrawing_header_hash)
            .ok_or(DaoError::InvalidHeader)?;
        if deposit_header.number() >= withdrawing_header.number() {
            return Err(DaoError::InvalidOutPoint.into());
        }

        let (deposit_ar, _, _, _) = extract_dao_data(deposit_header.dao())?;
        let (withdrawing_ar, _, _, _) = extract_dao_data(withdrawing_header.dao())?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_db::RocksDB;
    use ckb_store::{ChainDB, COLUMNS};
    use ckb_types::{
        bytes::Bytes,
        core::{
            capacity_bytes, cell::CellMetaBuilder, BlockBuilder, BlockNumber, EpochExt,
            HeaderBuilder, TransactionBuilder,
        },
        h256,
        utilities::DIFF_TWO,
        H256, U256,
    };

    fn new_store() -> ChainDB {
        ChainDB::new(RocksDB::open_tmp(COLUMNS), Default::default())
    }

    fn prepare_store(
        parent: &HeaderView,
        epoch_start: Option<BlockNumber>,
    ) -> (ChainDB, HeaderView) {
        let store = new_store();
        let txn = store.begin_transaction();

        let parent_block = BlockBuilder::default().header(parent.clone()).build();

        txn.insert_block(&parent_block).unwrap();
        txn.attach_block(&parent_block).unwrap();

        let epoch_ext = EpochExt::new_builder()
            .number(parent.number())
            .base_block_reward(Capacity::shannons(50_000_000_000))
            .remainder_reward(Capacity::shannons(1_000_128))
            .previous_epoch_hash_rate(U256::one())
            .last_block_hash_in_previous_epoch(h256!("0x1").pack())
            .start_number(epoch_start.unwrap_or_else(|| parent.number() - 1000))
            .length(2091)
            .compact_target(DIFF_TWO)
            .build();
        let epoch_hash = h256!("0x123455").pack();

        txn.insert_block_epoch_index(&parent.hash(), &epoch_hash)
            .unwrap();
        txn.insert_epoch_ext(&epoch_hash, &epoch_ext).unwrap();

        txn.commit().unwrap();

        (store, parent.clone())
    }

    #[test]
    fn check_dao_data_calculation() {
        let consensus = Consensus::default();

        let parent_number = 12345;
        let parent_header = HeaderBuilder::default()
            .number(parent_number.pack())
            .dao(pack_dao_data(
                10_000_000_000_123_456,
                Capacity::shannons(500_000_000_123_000),
                Capacity::shannons(400_000_000_123),
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&parent_header, None);
        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                10_000_586_990_682_998,
                Capacity::shannons(500_079_349_650_985),
                Capacity::shannons(429_314_308_674),
                Capacity::shannons(600_000_000_000)
            )
        );
    }

    #[test]
    fn check_initial_dao_data_calculation() {
        let consensus = Consensus::default();

        let parent_number = 0;
        let parent_header = HeaderBuilder::default()
            .number(parent_number.pack())
            .dao(pack_dao_data(
                10_000_000_000_000_000,
                Capacity::shannons(500_000_000_000_000),
                Capacity::shannons(400_000_000_000),
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&parent_header, Some(0));
        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                10_000_586_990_559_680,
                Capacity::shannons(500_079_349_527_985),
                Capacity::shannons(429_314_308_551),
                Capacity::shannons(600_000_000_000)
            )
        );
    }

    #[test]
    fn check_first_epoch_block_dao_data_calculation() {
        let consensus = Consensus::default();

        let parent_number = 12340;
        let parent_header = HeaderBuilder::default()
            .number(parent_number.pack())
            .dao(pack_dao_data(
                10_000_000_000_123_456,
                Capacity::shannons(500_000_000_123_000),
                Capacity::shannons(400_000_000_123),
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&parent_header, Some(12340));
        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                10_000_586_990_682_998,
                Capacity::shannons(500_079_349_650_985),
                Capacity::shannons(429_314_308_674),
                Capacity::shannons(600_000_000_000)
            )
        );
    }

    #[test]
    fn check_dao_data_calculation_overflows() {
        let consensus = Consensus::default();

        let parent_number = 12345;
        let parent_header = HeaderBuilder::default()
            .number(parent_number.pack())
            .dao(pack_dao_data(
                10_000_000_000_123_456,
                Capacity::shannons(18_446_744_073_709_000_000),
                Capacity::shannons(446_744_073_709),
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&parent_header, None);
        let result = DaoCalculator::new(&consensus, &store).dao_field(&[], &parent_header);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Internal(CapacityOverflow)"));
    }

    #[test]
    fn check_dao_data_calculation_with_transactions() {
        let consensus = Consensus::default();

        let parent_number = 12345;
        let parent_header = HeaderBuilder::default()
            .number(parent_number.pack())
            .dao(pack_dao_data(
                10_000_000_000_123_456,
                Capacity::shannons(500_000_000_123_000),
                Capacity::shannons(400_000_000_123),
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&parent_header, None);
        let input_cell_data = Bytes::from("abcde");
        let input_cell = CellOutput::new_builder()
            .capacity(capacity_bytes!(10000).pack())
            .build();
        let output_cell_data = Bytes::from("abcde12345");
        let output_cell = CellOutput::new_builder()
            .capacity(capacity_bytes!(20000).pack())
            .build();

        let tx = TransactionBuilder::default()
            .output(output_cell)
            .output_data(output_cell_data.pack())
            .build();
        let rtx = ResolvedTransaction {
            transaction: tx,
            resolved_cell_deps: vec![],
            resolved_inputs: vec![
                CellMetaBuilder::from_cell_output(input_cell, input_cell_data).build(),
            ],
            resolved_dep_groups: vec![],
        };

        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[rtx], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                10_000_586_990_682_998,
                Capacity::shannons(500_079_349_650_985),
                Capacity::shannons(429_314_308_674),
                Capacity::shannons(600_500_000_000)
            )
        );
    }

    #[test]
    fn check_withdraw_calculation() {
        let data = Bytes::from(vec![1; 10]);
        let output = CellOutput::new_builder()
            .capacity(capacity_bytes!(1000000).pack())
            .build();
        let tx = TransactionBuilder::default()
            .output(output)
            .output_data(data.pack())
            .build();
        let deposit_header = HeaderBuilder::default()
            .number(100.pack())
            .dao(pack_dao_data(
                10_000_000_000_123_456,
                Default::default(),
                Default::default(),
                Default::default(),
            ))
            .build();
        let deposit_block = BlockBuilder::default()
            .header(deposit_header)
            .transaction(tx.clone())
            .build();

        let out_point = OutPoint::new(tx.hash(), 0);

        let withdrawing_header = HeaderBuilder::default()
            .number(200.pack())
            .dao(pack_dao_data(
                10_000_000_001_123_456,
                Default::default(),
                Default::default(),
                Default::default(),
            ))
            .build();
        let withdrawing_block = BlockBuilder::default().header(withdrawing_header).build();

        let store = new_store();
        let txn = store.begin_transaction();
        txn.insert_block(&deposit_block).unwrap();
        txn.attach_block(&deposit_block).unwrap();
        txn.insert_block(&withdrawing_block).unwrap();
        txn.attach_block(&withdrawing_block).unwrap();
        txn.commit().unwrap();

        let consensus = Consensus::default();
        let calculator = DaoCalculator::new(&consensus, &store);
        let result = calculator.maximum_withdraw(&out_point, &withdrawing_block.hash());
        assert_eq!(result.unwrap(), Capacity::shannons(100_000_000_009_999));
    }

    #[test]
    fn check_withdraw_calculation_overflows() {
        let output = CellOutput::new_builder()
            .capacity(Capacity::shannons(18_446_744_073_709_550_000).pack())
            .build();
        let tx = TransactionBuilder::default().output(output).build();
        let deposit_header = HeaderBuilder::default()
            .number(100.pack())
            .dao(pack_dao_data(
                10_000_000_000_123_456,
                Default::default(),
                Default::default(),
                Default::default(),
            ))
            .build();
        let deposit_block = BlockBuilder::default()
            .header(deposit_header)
            .transaction(tx.clone())
            .build();

        let out_point = OutPoint::new(tx.hash(), 0);

        let withdrawing_header = HeaderBuilder::default()
            .number(200.pack())
            .dao(pack_dao_data(
                10_000_000_001_123_456,
                Default::default(),
                Default::default(),
                Default::default(),
            ))
            .build();
        let withdrawing_block = BlockBuilder::default()
            .header(withdrawing_header.clone())
            .build();

        let store = new_store();
        let txn = store.begin_transaction();
        txn.insert_block(&deposit_block).unwrap();
        txn.attach_block(&deposit_block).unwrap();
        txn.insert_block(&withdrawing_block).unwrap();
        txn.attach_block(&withdrawing_block).unwrap();
        txn.commit().unwrap();

        let consensus = Consensus::default();
        let calculator = DaoCalculator::new(&consensus, &store);
        let result = calculator.maximum_withdraw(&out_point, &withdrawing_header.hash());
        assert!(result.is_err());
    }
}
