use crate::{
    packed::{self, ProposalShortId, ProposalShortIdVec},
    prelude::*,
    vec::Vec,
};

#[test]
fn block_size_should_not_include_uncles_proposals() {
    let proposal1: ProposalShortId = [1; 10].into();
    let proposal2: ProposalShortId = [2; 10].into();
    let proposal3 = [3; 10].into();
    let proposals1: ProposalShortIdVec = vec![proposal1.clone()].into();
    let proposals2: ProposalShortIdVec = vec![proposal1.clone(), proposal2.clone()].into();
    let proposals3: ProposalShortIdVec = vec![proposal1, proposal2, proposal3].into();
    let uncle0 = packed::UncleBlock::new_builder().build();
    let uncle1 = packed::UncleBlock::new_builder()
        .proposals(proposals1)
        .build();
    let uncle2 = packed::UncleBlock::new_builder()
        .proposals(proposals2)
        .build();
    let uncle3 = packed::UncleBlock::new_builder()
        .proposals(proposals3)
        .build();
    let empty_uncles = vec![
        uncle0.clone(),
        uncle0.clone(),
        uncle0.clone(),
        uncle0.clone(),
    ];
    let uncles = vec![uncle0, uncle1, uncle2, uncle3];
    {
        // without block extension
        let mut empty_uncles = empty_uncles.clone();
        let mut uncles = uncles.clone();
        loop {
            let block_with_empty_uncles = packed::Block::new_builder()
                .uncles(empty_uncles.clone())
                .build();
            let block_with_uncles = packed::Block::new_builder().uncles(uncles.clone()).build();
            let actual = block_with_uncles.serialized_size_without_uncle_proposals();
            let actual_empty = block_with_empty_uncles.serialized_size_without_uncle_proposals();
            let expected = block_with_empty_uncles.as_slice().len();
            assert_eq!(actual, actual_empty);
            assert_eq!(actual, expected);
            if uncles.is_empty() {
                break;
            } else {
                empty_uncles.pop();
                uncles.pop();
            }
        }
    }
    {
        // with block extension
        let mut empty_uncles = empty_uncles;
        let mut uncles = uncles;
        let extensions: Vec<packed::Bytes> = vec![
            [0u8].into(),
            [0u8; 24].into(),
            [0u8; 48].into(),
            [0u8; 72].into(),
            [0u8; 96].into(),
        ];
        for extension in extensions {
            loop {
                let block_with_empty_uncles_v1 = packed::BlockV1::new_builder()
                    .uncles(empty_uncles.clone())
                    .extension(extension.clone())
                    .build();
                let block_with_empty_uncles = block_with_empty_uncles_v1.as_v0();
                let block_with_uncles = packed::BlockV1::new_builder()
                    .uncles(uncles.clone())
                    .extension(extension.clone())
                    .build()
                    .as_v0();
                let actual = block_with_uncles.serialized_size_without_uncle_proposals();
                let actual_empty =
                    block_with_empty_uncles.serialized_size_without_uncle_proposals();
                let expected_v1 = block_with_empty_uncles_v1.as_slice().len();
                let expected = block_with_empty_uncles.as_slice().len();
                assert_eq!(actual, actual_empty);
                assert_eq!(actual, expected);
                assert_eq!(expected_v1, expected);
                if uncles.is_empty() {
                    break;
                } else {
                    empty_uncles.pop();
                    uncles.pop();
                }
            }
        }
    }
}
