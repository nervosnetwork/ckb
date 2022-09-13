use std::collections::HashMap;

use ckb_types::{core::BlockNumber, U256};

use super::super::get_last_state_proof::FindBlocksViaDifficulties;

struct MockBlockSampler {
    blocks: HashMap<BlockNumber, U256>,
}

impl FindBlocksViaDifficulties for MockBlockSampler {
    fn get_block_total_difficulty(&self, number: BlockNumber) -> Option<U256> {
        self.blocks.get(&number).cloned()
    }
}

#[test]
fn test_find_blocks_via_difficulties() {
    let testcases = vec![
        (
            vec![(1u64, 10u64), (2, 20), (3, 30), (4, 40), (5, 50)],
            (1u64, 6u64, vec![10u64, 20, 30, 40, 50]),
            Some(vec![1u64, 2, 3, 4, 5]),
        ),
        (
            vec![(1u64, 10u64), (2, 20), (3, 30), (4, 40), (5, 50)],
            (1u64, 5u64, vec![10u64, 20, 30, 40, 50]),
            None,
        ),
        (
            vec![(1u64, 10u64), (2, 20), (3, 30), (4, 40), (5, 50)],
            (1u64, 6u64, vec![20, 30, 40]),
            Some(vec![2, 3, 4]),
        ),
        (
            vec![(2, 20), (3, 30), (4, 40), (5, 50)],
            (1u64, 6u64, vec![20, 30, 40]),
            None,
        ),
        (
            vec![(1u64, 10u64), (2, 20), (3, 30), (4, 40)],
            (1u64, 6u64, vec![20, 30, 40]),
            None,
        ),
        (
            vec![(1u64, 10u64), (2, 20), (3, 30), (4, 40), (5, 50)],
            (2u64, 5u64, vec![20, 30, 40]),
            Some(vec![2, 3, 4]),
        ),
        (
            vec![(1u64, 10u64), (2, 20), (3, 30), (4, 40), (5, 50)],
            (2u64, 5u64, vec![20, 30, 40, 50]),
            None,
        ),
        (
            vec![
                (1u64, 10u64),
                (2, 20),
                (3, 21),
                (4, 22),
                (5, 23),
                (6, 31),
                (7, 32),
                (8, 51),
                (9, 90),
                (10, 100),
            ],
            (
                1u64,
                11u64,
                vec![10u64, 20, 30, 40, 50, 60, 70, 80, 90, 100],
            ),
            Some(vec![1u64, 2, 6, 8, 9, 10]),
        ),
    ];
    for (block_diffs, (start, end, diffs), expected) in testcases {
        let sampler = MockBlockSampler {
            blocks: block_diffs
                .into_iter()
                .map(|(num, diff)| (num, U256::from(diff)))
                .collect(),
        };
        let diffs = diffs.into_iter().map(U256::from).collect::<Vec<_>>();
        let actual = sampler.get_block_numbers_via_difficulties(start, end, &diffs);
        if let Some(expected) = expected {
            assert!(actual.is_ok());
            assert_eq!(expected, actual.unwrap());
        } else {
            assert!(actual.is_err());
        }
    }
}
