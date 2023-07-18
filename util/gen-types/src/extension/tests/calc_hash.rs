use crate::{h256, packed, prelude::*};
use ckb_hash::blake2b_256;

#[test]
fn proposals_hash() {
    let proposal1 = [1; 10].pack();
    let proposal2 = [2; 10].pack();
    let proposals = vec![proposal1, proposal2].pack();
    let expect = h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562");
    assert_eq!(proposals.calc_proposals_hash(), expect.pack());
}

#[test]
fn empty_proposals_hash() {
    let proposals = packed::ProposalShortIdVec::new_builder().build();
    let expect = h256!("0x0");
    assert_eq!(proposals.calc_proposals_hash(), expect.pack());
}

#[test]
fn uncles_hash() {
    let uncle1_raw_header = packed::RawHeader::new_builder()
        .version(0u32.pack())
        .compact_target(0x1e08_3126u32.pack())
        .timestamp(0x5cd2_b117u64.pack())
        .number(0x400u64.pack())
        .epoch(0x0007_0800_1800_0001u64.pack())
        .parent_hash(
            h256!("0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831").pack(),
        )
        .transactions_root(
            h256!("0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f").pack(),
        )
        .proposals_hash(
            h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").pack(),
        )
        .extra_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .dao(h256!("0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000").pack())
        .build();
    let uncle1_header = packed::Header::new_builder()
        .raw(uncle1_raw_header)
        .nonce(0x5ff1_389a_f870_6543_11a2_bee6_1237u128.pack())
        .build();
    let uncle1_proposals = vec![[1; 10].pack(), [2; 10].pack()].pack();
    let uncle1 = packed::UncleBlock::new_builder()
        .header(uncle1_header)
        .proposals(uncle1_proposals)
        .build();

    let uncle2_raw_header = packed::RawHeader::new_builder()
        .version(0u32.pack())
        .compact_target(0x2001_0000u32.pack())
        .timestamp(0x5cd2_1a16u64.pack())
        .number(0x400u64.pack())
        .epoch(0x0007_0800_1800_0001u64.pack())
        .parent_hash(
            h256!("0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831").pack(),
        )
        .transactions_root(
            h256!("0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f").pack(),
        )
        .proposals_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .extra_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .dao(h256!("0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000").pack())
        .build();
    let uncle2_header = packed::Header::new_builder()
        .raw(uncle2_raw_header)
        .nonce(0x2f39_2d41_cd70_024fu128.pack())
        .build();
    let uncle2 = packed::UncleBlock::new_builder()
        .header(uncle2_header)
        .build();

    let uncles = vec![uncle1, uncle2].pack();
    let expect = h256!("0x0135d01f169a870bd9c92b2b37aecfa0fbfb7c1862cc176e03bb525fab0649d9");
    assert_eq!(uncles.calc_uncles_hash(), expect.pack());
}

#[test]
fn empty_uncles_hash() {
    let uncles = packed::UncleBlockVec::new_builder().build();
    let expect = h256!("0x0");
    assert_eq!(uncles.calc_uncles_hash(), expect.pack());
}

#[test]
fn empty_extra_hash() {
    let block = packed::Block::new_builder().build();
    let expect = h256!("0x0");
    assert_eq!(block.calc_extra_hash().extra_hash(), expect.pack());
}

#[test]
fn empty_script_hash() {
    let script = packed::Script::new_builder().build();
    let expect = h256!("0x77c93b0632b5b6c3ef922c5b7cea208fb0a7c427a13d50e13d3fefad17e0c590");
    assert_eq!(script.calc_script_hash(), expect.pack());
}

#[test]
fn always_success_script_hash() {
    let always_success = include_bytes!("../../../../../script/testdata/always_success");
    let always_success_hash = blake2b_256(&always_success[..]);

    let script = packed::Script::new_builder()
        .code_hash(always_success_hash.pack())
        .build();
    let expect = h256!("0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412");
    assert_eq!(script.calc_script_hash(), expect.pack());
}

#[test]
fn one_arg_script_hash() {
    let script = packed::Script::new_builder().args(vec![1].pack()).build();
    let expect = h256!("0x67951b34bce20cb71b7e235c1f8cda259628d99d94825bffe549c23b4dd2930f");
    assert_eq!(script.calc_script_hash(), expect.pack());
}
