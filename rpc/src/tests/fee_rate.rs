use crate::util::{FeeRateCollector, FeeRateProvider};
use ckb_jsonrpc_types::FeeRateStatistics;
use ckb_types::core::{BlockExt, BlockNumber, Capacity};
use std::collections::HashMap;

struct DummyFeeRateProvider {
    tip_number: BlockNumber,
    block_exts: HashMap<BlockNumber, BlockExt>,
    max_target: u64,
}

impl DummyFeeRateProvider {
    pub fn new(max_target: u64) -> DummyFeeRateProvider {
        DummyFeeRateProvider {
            tip_number: 0,
            block_exts: HashMap::new(),
            max_target,
        }
    }

    pub fn append(&mut self, number: BlockNumber, ext: BlockExt) {
        if number > self.tip_number {
            self.tip_number = number;
        }
        self.block_exts.insert(number, ext);
    }

    pub fn set_max_target(&mut self, max_target: u64) {
        self.max_target = max_target
    }
}

impl FeeRateProvider for DummyFeeRateProvider {
    fn get_tip_number(&self) -> BlockNumber {
        self.tip_number
    }

    fn get_block_ext_by_number(&self, number: BlockNumber) -> Option<BlockExt> {
        self.block_exts.get(&number).cloned()
    }

    fn max_target(&self) -> u64 {
        self.max_target
    }
}

#[test]
fn test_fee_rate_statics() {
    let mut provider = DummyFeeRateProvider::new(30);
    for i in 0..=21 {
        let ext = BlockExt {
            received_at: 0,
            total_difficulty: 0u64.into(),
            total_uncles_count: 0,
            verified: None,
            txs_fees: vec![Capacity::shannons(i * i * 100)],
            cycles: Some(vec![i * 100]),
            txs_sizes: Some(vec![i * 100]),
        };
        provider.append(i, ext);
    }

    let statistics = FeeRateCollector::new(&provider).statistics(None);
    assert_eq!(
        statistics,
        Some(FeeRateStatistics {
            mean: 11_000.into(),
            median: 11_000.into(),
        })
    );

    let statistics = FeeRateCollector::new(&provider).statistics(Some(9));
    assert_eq!(
        statistics,
        Some(FeeRateStatistics {
            mean: 17_000.into(),
            median: 17_000.into()
        })
    );

    let statistics = FeeRateCollector::new(&provider).statistics(Some(30));
    assert_eq!(
        statistics,
        Some(FeeRateStatistics {
            mean: 11_000.into(),
            median: 11_000.into(),
        })
    );

    let statistics = FeeRateCollector::new(&provider).statistics(Some(0));
    assert_eq!(
        statistics,
        Some(FeeRateStatistics {
            mean: 21_000.into(),
            median: 21_000.into(),
        })
    );

    provider.set_max_target(10);
    let statistics11 = FeeRateCollector::new(&provider).statistics(Some(11));
    let statistics12 = FeeRateCollector::new(&provider).statistics(Some(12));
    assert_eq!(statistics11, statistics12);
    assert_eq!(
        statistics11,
        Some(FeeRateStatistics {
            mean: 16500.into(),
            median: 16500.into(),
        })
    );
}
