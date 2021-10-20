use crate::{packed, prelude::*};

#[test]
fn block_size_should_not_include_uncles_proposals() {
    let proposal1 = [1; 10].pack();
    let proposal2 = [2; 10].pack();
    let proposal3 = [3; 10].pack();
    let proposals1 = vec![proposal1.clone()].pack();
    let proposals2 = vec![proposal1.clone(), proposal2.clone()].pack();
    let proposals3 = vec![proposal1, proposal2, proposal3].pack();
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
                .uncles(empty_uncles.clone().pack())
                .build();
            let block_with_uncles = packed::Block::new_builder()
                .uncles(uncles.clone().pack())
                .build();
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
            vec![0u8].pack(),
            vec![0u8; 24].pack(),
            vec![0u8; 48].pack(),
            vec![0u8; 72].pack(),
            vec![0u8; 96].pack(),
        ];
        for extension in extensions {
            loop {
                let block_with_empty_uncles_v1 = packed::BlockV1::new_builder()
                    .uncles(empty_uncles.clone().pack())
                    .extension(extension.clone())
                    .build();
                let block_with_empty_uncles = block_with_empty_uncles_v1.as_v0();
                let block_with_uncles = packed::BlockV1::new_builder()
                    .uncles(uncles.clone().pack())
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
