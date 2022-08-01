use ckb_resource::Resource;
use ckb_types::{core::Capacity, packed, prelude::*, H256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{build_genesis_epoch_ext, ChainSpec, Params};

mod consensus;
mod versionbits;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SystemCell {
    pub path: String,
    pub index: usize,
    pub data_hash: H256,
    pub type_hash: Option<H256>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DepGroups {
    pub included_cells: Vec<String>,
    pub tx_hash: H256,
    pub index: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SpecHashes {
    pub genesis: H256,
    pub cellbase: H256,
    pub system_cells: Vec<SystemCell>,
    pub dep_groups: Vec<DepGroups>,
}

fn load_spec_by_name(name: &str) -> ChainSpec {
    let res = if name == "ckb" {
        Resource::bundled("specs/mainnet.toml".to_string())
    } else {
        let base_name = &name[4..];
        Resource::bundled(format!("specs/{}.toml", base_name))
    };

    ChainSpec::load_from(&res).expect("load spec by name")
}

#[test]
fn test_bundled_specs() {
    let bundled_spec_err: &str = r#"
            Unmatched Bundled Spec.

            Forget to generate docs/hashes.toml? Try to run;

                ckb list-hashes -b > docs/hashes.toml
        "#;

    let spec_hashes: HashMap<String, SpecHashes> =
        toml::from_str(include_str!("../../../docs/hashes.toml")).unwrap();

    for (name, spec_hashes) in spec_hashes.iter() {
        let spec = load_spec_by_name(name);
        assert_eq!(name, &spec.name, "{}", bundled_spec_err);
        if let Some(genesis_hash) = &spec.genesis.hash {
            assert_eq!(genesis_hash, &spec_hashes.genesis, "{}", bundled_spec_err);
        }

        let consensus = spec.build_consensus();
        if let Err(err) = consensus {
            panic!("{}", err);
        }
        let consensus = consensus.unwrap();
        let block = consensus.genesis_block();
        let cellbase = block.transaction(0).unwrap();
        let cellbase_hash: H256 = cellbase.hash().unpack();

        assert_eq!(spec_hashes.cellbase, cellbase_hash);

        let mut system_cells = HashMap::new();
        for (index_minus_one, (cell, (output, data))) in spec_hashes
            .system_cells
            .iter()
            .zip(
                cellbase
                    .outputs()
                    .into_iter()
                    .zip(cellbase.outputs_data().into_iter())
                    .skip(1),
            )
            .enumerate()
        {
            let data_hash: H256 = packed::CellOutput::calc_data_hash(&data.raw_data()).unpack();
            let type_hash: Option<H256> = output
                .type_()
                .to_opt()
                .map(|script| script.calc_script_hash().unpack());
            assert_eq!(index_minus_one + 1, cell.index, "{}", bundled_spec_err);
            assert_eq!(cell.data_hash, data_hash, "{}", bundled_spec_err);
            assert_eq!(cell.type_hash, type_hash, "{}", bundled_spec_err);
            system_cells.insert(cell.index, cell.path.as_str());
        }

        // dep group tx should be the first tx except cellbase
        let dep_group_tx = block.transaction(1).unwrap();

        // input index of dep group tx
        let dep_group_tx_input_index = system_cells.len() + 1;
        let input_capacity: Capacity = cellbase
            .output(dep_group_tx_input_index)
            .unwrap()
            .capacity()
            .unpack();
        let outputs_capacity = dep_group_tx
            .outputs()
            .into_iter()
            .map(|output| Unpack::<Capacity>::unpack(&output.capacity()))
            .try_fold(Capacity::zero(), Capacity::safe_add)
            .unwrap();
        // capacity for input and outpus should be same
        assert_eq!(input_capacity, outputs_capacity);

        // dep group tx has only one input
        assert_eq!(dep_group_tx.inputs().len(), 1);

        // all dep groups should be in the spec file
        assert_eq!(
            dep_group_tx.outputs_data().len(),
            spec_hashes.dep_groups.len(),
            "{}",
            bundled_spec_err
        );

        for (i, output_data) in dep_group_tx.outputs_data().into_iter().enumerate() {
            let dep_group = &spec_hashes.dep_groups[i];

            // check the tx hashes of dep groups in spec file
            let tx_hash = dep_group.tx_hash.pack();
            assert_eq!(tx_hash, dep_group_tx.hash(), "{}", bundled_spec_err);

            let out_point_vec = packed::OutPointVec::from_slice(&output_data.raw_data()).unwrap();

            // all cells included by a dep group should be list in the spec file
            assert_eq!(
                out_point_vec.len(),
                dep_group.included_cells.len(),
                "{}",
                bundled_spec_err
            );

            for (j, out_point) in out_point_vec.into_iter().enumerate() {
                let dep_path = &dep_group.included_cells[j];

                // dep groups out_point should point to cellbase
                assert_eq!(cellbase.hash(), out_point.tx_hash(), "{}", bundled_spec_err);

                let index_in_cellbase: usize = out_point.index().unpack();

                // check index for included cells in dep groups
                assert_eq!(
                    system_cells[&index_in_cellbase], dep_path,
                    "{}",
                    bundled_spec_err
                );
            }
        }
    }
}

#[test]
fn test_default_params() {
    let params: Params = toml::from_str("").unwrap();
    let expected = Params::default();
    assert_eq!(params, expected);

    let test_params: &str = r#"
            genesis_epoch_length = 100
        "#;

    let params: Params = toml::from_str(test_params).unwrap();
    let expected = Params {
        genesis_epoch_length: Some(100),
        ..Default::default()
    };

    assert_eq!(params, expected);

    let test_params: &str = r#"
            max_block_bytes = 100
        "#;

    let params: Params = toml::from_str(test_params).unwrap();
    let expected = Params {
        max_block_bytes: Some(100),
        ..Default::default()
    };

    assert_eq!(params, expected);

    let test_params: &str = r#"
            max_block_proposals_limit = 100
        "#;

    let params: Params = toml::from_str(test_params).unwrap();
    let expected = Params {
        max_block_proposals_limit: Some(100),
        ..Default::default()
    };

    assert_eq!(params, expected);

    let test_params: &str = r#"
            orphan_rate_target = [1, 40]
        "#;

    let params: Params = toml::from_str(test_params).unwrap();
    let expected = Params {
        orphan_rate_target: Some((1, 40)),
        ..Default::default()
    };

    assert_eq!(params, expected);
}

#[test]
fn test_params_skip_serializing_if_option_is_none() {
    let default = Params::default();

    let serialized: Vec<u8> = toml::to_vec(&default).unwrap();
    let except: Vec<u8> = vec![];

    assert_eq!(serialized, except);
}

#[test]
fn test_default_genesis_epoch_ext() {
    use ckb_types::core::EpochExt;
    use ckb_types::{packed, U256};

    let params = Params::default();
    let compact_target = 0x1a08a97e;
    let genesis_epoch_length = 1743;

    let genesis_epoch_ext = build_genesis_epoch_ext(
        params.initial_primary_epoch_reward(),
        compact_target,
        genesis_epoch_length,
        params.epoch_duration_target(),
        params.orphan_rate_target(),
    );

    // hard code mainnet
    let expected = EpochExt::new_builder()
        .number(0)
        .base_block_reward(Capacity::shannons(110029157726))
        .remainder_reward(Capacity::shannons(1390))
        .previous_epoch_hash_rate(U256::from(0x3aa602ee1f497u64))
        .last_block_hash_in_previous_epoch(packed::Byte32::zero())
        .start_number(0)
        .length(genesis_epoch_length)
        .compact_target(compact_target)
        .build();

    assert_eq!(genesis_epoch_ext, expected);
}
