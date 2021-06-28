use crate::{genesis_verifier::GenesisVerifier, NumberError, UnknownParentError};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_error::assert_error_eq;
use ckb_types::{core::EpochNumberWithFraction, prelude::*};
use ckb_verification_traits::Verifier;

#[test]
pub fn test_genesis_non_zero_number() {
    let genesis_block = Consensus::default().genesis_block().to_owned();
    let genesis_block = genesis_block
        .as_advanced_builder()
        .number(42.pack())
        .epoch(EpochNumberWithFraction::from_full_value(0).pack())
        .build();
    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    let verifier = GenesisVerifier::new();
    assert_error_eq!(
        verifier.verify(&consensus).unwrap_err(),
        NumberError {
            expected: 0,
            actual: 42
        },
    );
}

#[test]
pub fn test_genesis_non_zero_parent_hash() {
    let genesis_block = Consensus::default().genesis_block().to_owned();
    let genesis_block = genesis_block
        .as_advanced_builder()
        .parent_hash([42u8; 32].pack())
        .build();
    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    let verifier = GenesisVerifier::new();
    assert_error_eq!(
        verifier.verify(&consensus).unwrap_err(),
        UnknownParentError {
            parent_hash: [42u8; 32].pack()
        },
    );
}

#[test]
pub fn test_default_genesis() {
    let consensus = ConsensusBuilder::default().build();
    let verifier = GenesisVerifier::new();
    verifier.verify(&consensus).expect("pass verification");
}

#[test]
pub fn test_chain_specs() {
    use ckb_chain_spec::ChainSpec;
    use ckb_resource::{Resource, AVAILABLE_SPECS};
    fn load_spec_by_name(name: &str) -> ChainSpec {
        let res = Resource::bundled(format!("specs/{}.toml", name));
        ChainSpec::load_from(&res).expect("load spec by name")
    }
    for name in AVAILABLE_SPECS {
        let spec = load_spec_by_name(name);
        let consensus = spec.build_consensus().expect("build consensus");
        let verifier = GenesisVerifier::new();
        verifier.verify(&consensus).expect("pass verification");
    }
}
