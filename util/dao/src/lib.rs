use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::{extract_dao_data, pack_dao_data, Error};
use ckb_resource::CODE_HASH_DAO;
use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainStore};
use ckb_types::{
    core::{cell::ResolvedTransaction, BlockNumber, Capacity, EpochExt, HeaderView},
    packed::{Byte32, CellOutput, OutPoint},
    prelude::*,
    H256,
};
use failure::Error as FailureError;
use std::cmp::max;
use std::collections::HashSet;

pub struct DaoCalculator<'a, CS, DL> {
    pub consensus: &'a Consensus,
    pub store: &'a CS,
    pub data_loader: DL,
}

impl<'a, CS: ChainStore<'a>> DaoCalculator<'a, CS, DataLoaderWrapper<'a, CS>> {
    pub fn new(consensus: &'a Consensus, store: &'a CS) -> Self {
        let data_loader = DataLoaderWrapper::new(store);
        DaoCalculator {
            consensus,
            store,
            data_loader,
        }
    }

    pub fn primary_block_reward(&self, target: &HeaderView) -> Result<Capacity, FailureError> {
        let target_epoch = self
            .store
            .get_block_epoch_index(&target.hash())
            .and_then(|index| self.store.get_epoch_ext(&index))
            .ok_or(Error::InvalidHeader)?;

        target_epoch.block_reward(target.number())
    }

    pub fn secondary_block_reward(&self, target: &HeaderView) -> Result<Capacity, FailureError> {
        if target.number() == 0 {
            return Ok(Capacity::zero());
        }
        let target_parent_hash = target.data().raw().parent_hash();
        let target_parent = self
            .store
            .get_block_header(&target_parent_hash)
            .ok_or(Error::InvalidHeader)?;
        let target_epoch = self
            .store
            .get_block_epoch_index(&target.hash())
            .and_then(|index| self.store.get_epoch_ext(&index))
            .ok_or(Error::InvalidHeader)?;

        let target_g2 = calculate_g2(
            target.number(),
            &target_epoch,
            self.consensus.secondary_epoch_reward(),
        )?;
        let (_, _, target_parent_u) = extract_dao_data(target_parent.dao())?;
        let (_, target_c, _) = extract_dao_data(target.dao())?;
        let reward = u128::from(target_g2.as_u64()) * u128::from(target_parent_u.as_u64())
            / (max(u128::from(target_c.as_u64()), 1));
        Ok(Capacity::shannons(reward as u64))
    }

    // Notice unlike primary_block_reward and secondary_epoch_reward above,
    // this starts calculating from parent, not target header.
    pub fn base_block_reward(&self, parent: &HeaderView) -> Result<Capacity, FailureError> {
        let target_number = self
            .consensus
            .finalize_target(parent.number() + 1)
            .ok_or(Error::InvalidHeader)?;
        let target = self
            .store
            .get_block_hash(target_number)
            .and_then(|hash| self.store.get_block_header(&hash))
            .ok_or(Error::InvalidHeader)?;

        let primary_block_reward = self.primary_block_reward(&target)?;
        let secondary_block_reward = self.secondary_block_reward(&target)?;

        primary_block_reward
            .safe_add(secondary_block_reward)
            .map_err(Into::into)
    }

    pub fn dao_field(
        &self,
        rtxs: &[ResolvedTransaction],
        parent: &HeaderView,
    ) -> Result<Byte32, FailureError> {
        // Freed occupied capacities from consumed inputs
        let freed_occupied_capacities =
            rtxs.iter().try_fold(Capacity::zero(), |capacities, rtx| {
                self.input_occupied_capacities(rtx)
                    .and_then(|c| capacities.safe_add(c).map_err(Into::into))
            })?;

        // Newly added occupied capacities from outputs
        let added_occupied_capacities =
            rtxs.iter().try_fold(Capacity::zero(), |capacities, rtx| {
                rtx.transaction
                    .outputs_with_data_iter()
                    .try_fold(Capacity::zero(), |tx_capacities, (output, data)| {
                        Capacity::bytes(data.len()).and_then(|data_capacity| {
                            output
                                .occupied_capacity(data_capacity)
                                .and_then(|c| tx_capacities.safe_add(c))
                        })
                    })
                    .and_then(|c| capacities.safe_add(c))
            })?;

        let (parent_ar, parent_c, parent_u) = extract_dao_data(parent.dao())?;

        let (parent_g, parent_g2) = if parent.number() == 0 {
            (Capacity::zero(), Capacity::zero())
        } else {
            let target_number = self
                .consensus
                .finalize_target(parent.number())
                .ok_or(Error::InvalidHeader)?;
            let target = self
                .store
                .get_block_hash(target_number)
                .and_then(|hash| self.store.get_block_header(&hash))
                .ok_or(Error::InvalidHeader)?;
            let target_epoch = self
                .store
                .get_block_epoch_index(&target.hash())
                .and_then(|index| self.store.get_epoch_ext(&index))
                .ok_or(Error::InvalidHeader)?;
            let parent_g2 = calculate_g2(
                target.number(),
                &target_epoch,
                self.consensus.secondary_epoch_reward(),
            )?;
            let parent_g = self
                .primary_block_reward(&target)
                .and_then(|c| c.safe_add(parent_g2).map_err(Into::into))?;
            (parent_g, parent_g2)
        };

        let current_c = parent_c.safe_add(parent_g)?;
        let current_ar = u128::from(parent_ar)
            * u128::from((parent_c.safe_add(parent_g2)?).as_u64())
            / (max(u128::from(parent_c.as_u64()), 1));
        let current_u = parent_u
            .safe_add(added_occupied_capacities)
            .and_then(|u| u.safe_sub(freed_occupied_capacities))?;

        Ok(pack_dao_data(current_ar as u64, current_c, current_u))
    }

    pub fn maximum_withdraw(
        &self,
        out_point: &OutPoint,
        withdraw_header_hash: &Byte32,
    ) -> Result<Capacity, FailureError> {
        let (tx, block_hash) = self
            .store
            .get_transaction(&out_point.tx_hash())
            .ok_or(Error::InvalidOutPoint)?;
        let output = tx
            .outputs()
            .get(out_point.index().unpack())
            .ok_or(Error::InvalidOutPoint)?;
        let output_data = tx
            .outputs_data()
            .get(out_point.index().unpack())
            .ok_or(Error::InvalidOutPoint)?;
        self.calculate_maximum_withdraw(
            &output,
            Capacity::bytes(output_data.len())?,
            &block_hash,
            withdraw_header_hash,
        )
    }

    pub fn transaction_fee(&self, rtx: &ResolvedTransaction) -> Result<Capacity, FailureError> {
        let header_deps: HashSet<Byte32> = rtx.transaction.header_deps_iter().collect();
        rtx.resolved_inputs
            .iter()
            .enumerate()
            .try_fold(Capacity::zero(), |capacities, (i, cell_meta)| {
                let capacity: Result<Capacity, FailureError> = {
                    let output = &cell_meta.cell_output;
                    if output
                        .type_()
                        .to_opt()
                        .map(|t| Unpack::<H256>::unpack(&t.code_hash()) == CODE_HASH_DAO)
                        .unwrap_or(false)
                    {
                        let deposit_header_hash = cell_meta
                            .transaction_info
                            .as_ref()
                            .map(|info| &info.block_hash)
                            .filter(|hash| header_deps.contains(&hash.pack()))
                            .ok_or(Error::InvalidOutPoint)?;
                        let withdraw_header_hash = rtx
                            .transaction
                            .witnesses()
                            .get(i)
                            .and_then(|witness| witness.get(1))
                            .ok_or(Error::InvalidOutPoint)
                            .and_then(|witness_data| {
                                if witness_data.raw_data().len() != 8 {
                                    Err(Error::Format)
                                } else {
                                    Ok(LittleEndian::read_u64(&witness_data.raw_data()[0..8]))
                                }
                            })
                            .and_then(|header_dep_index| {
                                rtx.transaction
                                    .header_deps()
                                    .get(header_dep_index as usize)
                                    .and_then(|hash| header_deps.get(&hash))
                                    .ok_or(Error::InvalidOutPoint)
                            })?;
                        self.calculate_maximum_withdraw(
                            &output,
                            Capacity::bytes(cell_meta.data_bytes as usize)?,
                            &deposit_header_hash.pack(),
                            &withdraw_header_hash,
                        )
                    } else {
                        Ok(output.capacity().unpack())
                    }
                };
                capacity.and_then(|c| c.safe_add(capacities).map_err(Into::into))
            })
            .and_then(|x| {
                rtx.transaction
                    .outputs_capacity()
                    .and_then(|y| x.safe_sub(y))
                    .map_err(Into::into)
            })
    }

    fn input_occupied_capacities(
        &self,
        rtx: &ResolvedTransaction,
    ) -> Result<Capacity, FailureError> {
        rtx.resolved_inputs
            .iter()
            .try_fold(Capacity::zero(), |capacities, cell_meta| {
                let current_capacity = cell_meta.occupied_capacity();
                current_capacity.and_then(|c| capacities.safe_add(c))
            })
            .map_err(Into::into)
    }

    fn calculate_maximum_withdraw(
        &self,
        output: &CellOutput,
        output_data_capacity: Capacity,
        deposit_header_hash: &Byte32,
        withdraw_header_hash: &Byte32,
    ) -> Result<Capacity, FailureError> {
        let deposit_header = self
            .store
            .get_block_header(deposit_header_hash)
            .ok_or(Error::InvalidHeader)?;
        let withdraw_header = self
            .store
            .get_block_header(withdraw_header_hash)
            .ok_or(Error::InvalidHeader)?;
        let (deposit_ar, _, _) = extract_dao_data(deposit_header.dao())?;
        let (withdraw_ar, _, _) = extract_dao_data(withdraw_header.dao())?;

        let occupied_capacity = output.occupied_capacity(output_data_capacity)?;
        let output_capacity: Capacity = output.capacity().unpack();
        let counted_capacity = output_capacity.safe_sub(occupied_capacity)?;
        let withdraw_counted_capacity = u128::from(counted_capacity.as_u64())
            * u128::from(withdraw_ar)
            / u128::from(deposit_ar);
        let withdraw_capacity =
            Capacity::shannons(withdraw_counted_capacity as u64).safe_add(occupied_capacity)?;

        Ok(withdraw_capacity)
    }
}

fn calculate_g2(
    block_number: BlockNumber,
    current_epoch_ext: &EpochExt,
    secondary_epoch_reward: Capacity,
) -> Result<Capacity, FailureError> {
    if block_number == 0 {
        return Ok(Capacity::zero());
    }
    let epoch_length = current_epoch_ext.length();
    let mut g2 = Capacity::shannons(secondary_epoch_reward.as_u64() / epoch_length);
    let remainder = secondary_epoch_reward.as_u64() % epoch_length;
    if block_number >= current_epoch_ext.start_number()
        && block_number < current_epoch_ext.start_number() + remainder
    {
        g2 = g2.safe_add(Capacity::one())?;
    }
    Ok(g2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_db::RocksDB;
    use ckb_store::{ChainDB, COLUMNS};
    use ckb_types::{
        bytes::Bytes,
        core::{
            capacity_bytes, cell::CellMetaBuilder, BlockBuilder, BlockNumber, HeaderBuilder,
            TransactionBuilder,
        },
        h256, H256, U256,
    };

    fn new_store() -> ChainDB {
        ChainDB::new(RocksDB::open_tmp(COLUMNS))
    }

    fn prepare_store(
        consensus: &Consensus,
        parent: &HeaderView,
        target_epoch_start: Option<BlockNumber>,
    ) -> (ChainDB, HeaderView) {
        let store = new_store();
        let txn = store.begin_transaction();

        if let Some(target_number) = consensus.finalize_target(parent.number()) {
            let target_epoch_start = target_epoch_start.unwrap_or(target_number - 300);
            let mut index = HeaderBuilder::default()
                .number((target_epoch_start - 1).pack())
                .build();
            // TODO: should make it simple after refactor get_ancestor
            for number in target_epoch_start..parent.number() {
                let epoch_ext = EpochExt::new(
                    number,
                    Capacity::shannons(50_000_000_000),
                    Capacity::shannons(1_000_128),
                    U256::one(),
                    h256!("0x1"),
                    target_epoch_start,
                    2091,
                    U256::from(1u64),
                );

                let header = HeaderBuilder::default()
                    .number(number.pack())
                    .parent_hash(index.hash())
                    .build();
                let block = BlockBuilder::default().header(header.clone()).build();

                index = header.clone();

                txn.insert_block(&block).unwrap();
                txn.attach_block(&block).unwrap();
                txn.insert_block_epoch_index(&header.hash(), &header.hash())
                    .unwrap();
                txn.insert_epoch_ext(&header.hash(), &epoch_ext).unwrap();
            }

            let parent = parent
                .as_advanced_builder()
                .parent_hash(index.hash())
                .build();
            let parent_block = BlockBuilder::default().header(parent.clone()).build();

            txn.insert_block(&parent_block).unwrap();
            txn.attach_block(&parent_block).unwrap();

            txn.commit().unwrap();

            return (store, parent.clone());
        } else {
            let parent_block = BlockBuilder::default().header(parent.clone()).build();
            txn.insert_block(&parent_block).unwrap();
            txn.attach_block(&parent_block).unwrap();

            txn.commit().unwrap();

            return (store, parent.clone());
        }
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
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&consensus, &parent_header, None);
        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                10_000_573_888_215_141,
                Capacity::shannons(500_078_694_527_592),
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
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&consensus, &parent_header, None);
        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                10_000_000_000_000_000,
                Capacity::shannons(500_000_000_000_000),
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
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&consensus, &parent_header, Some(12329));
        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                10_000_573_888_215_161,
                Capacity::shannons(500_078_694_527_593),
                Capacity::shannons(600_000_000_000)
            )
        );
    }

    #[test]
    fn check_dao_data_calculation_works_on_zero_initial_capacity() {
        let consensus = Consensus::default();

        let parent_number = 0;
        let parent_header = HeaderBuilder::default()
            .number(parent_number.pack())
            .dao(pack_dao_data(
                10_000_000_000_000_000,
                Capacity::shannons(0),
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&consensus, &parent_header, None);
        let result = DaoCalculator::new(&consensus, &store)
            .dao_field(&[], &parent_header)
            .unwrap();
        let dao_data = extract_dao_data(result).unwrap();
        assert_eq!(
            dao_data,
            (
                0,
                Capacity::shannons(0),
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
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&consensus, &parent_header, None);
        let result = DaoCalculator::new(&consensus, &store).dao_field(&[], &parent_header);
        assert!(result.is_err());
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
                Capacity::shannons(600_000_000_000),
            ))
            .build();

        let (store, parent_header) = prepare_store(&consensus, &parent_header, None);
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
            transaction: &tx,
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
                10_000_573_888_215_141,
                Capacity::shannons(500_078_694_527_592),
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
            ))
            .build();
        let deposit_block = BlockBuilder::default()
            .header(deposit_header)
            .transaction(tx.clone())
            .build();

        let out_point = OutPoint::new(tx.hash().unpack(), 0);

        let withdraw_header = HeaderBuilder::default()
            .number(200.pack())
            .dao(pack_dao_data(
                10_000_000_001_123_456,
                Default::default(),
                Default::default(),
            ))
            .build();
        let withdraw_block = BlockBuilder::default()
            .header(withdraw_header.clone())
            .build();

        let store = new_store();
        let txn = store.begin_transaction();
        txn.insert_block(&deposit_block).unwrap();
        txn.attach_block(&deposit_block).unwrap();
        txn.insert_block(&withdraw_block).unwrap();
        txn.attach_block(&withdraw_block).unwrap();
        txn.commit().unwrap();

        let consensus = Consensus::default();
        let calculator = DaoCalculator::new(&consensus, &store);
        let result = calculator.maximum_withdraw(&out_point, &withdraw_header.hash());
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
            ))
            .build();
        let deposit_block = BlockBuilder::default()
            .header(deposit_header.clone())
            .transaction(tx.clone())
            .build();

        let out_point = OutPoint::new(tx.hash().unpack(), 0);

        let withdraw_header = HeaderBuilder::default()
            .number(200.pack())
            .dao(pack_dao_data(
                10_000_000_001_123_456,
                Default::default(),
                Default::default(),
            ))
            .build();
        let withdraw_block = BlockBuilder::default()
            .header(withdraw_header.clone())
            .build();

        let store = new_store();
        let txn = store.begin_transaction();
        txn.insert_block(&deposit_block).unwrap();
        txn.attach_block(&deposit_block).unwrap();
        txn.insert_block(&withdraw_block).unwrap();
        txn.attach_block(&withdraw_block).unwrap();
        txn.commit().unwrap();

        let consensus = Consensus::default();
        let calculator = DaoCalculator::new(&consensus, &store);
        let result = calculator.maximum_withdraw(&out_point, &withdraw_header.hash());
        assert!(result.is_err());
    }
}
