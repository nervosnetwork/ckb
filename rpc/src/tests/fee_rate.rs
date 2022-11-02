use crate::util::{FeeRateCollector, FeeRateProvider};
use ckb_jsonrpc_types::FeeRateStatics;
use ckb_types::core::{BlockExt, BlockNumber, Capacity};
use std::collections::HashMap;

struct DummyFeeRateProvider {
    tip_number: BlockNumber,
    block_exts: HashMap<BlockNumber, BlockExt>,
}

impl DummyFeeRateProvider {
    pub fn new() -> DummyFeeRateProvider {
        DummyFeeRateProvider {
            tip_number: 0,
            block_exts: HashMap::new(),
        }
    }

    pub fn append(&mut self, number: BlockNumber, ext: BlockExt) {
        if number > self.tip_number {
            self.tip_number = number;
        }
        self.block_exts.insert(number, ext);
    }
}

impl FeeRateProvider for DummyFeeRateProvider {
    fn get_tip_number(&self) -> BlockNumber {
        self.tip_number
    }

    fn get_block_ext_by_number(&self, number: BlockNumber) -> Option<BlockExt> {
        self.block_exts.get(&number).cloned()
    }
}

#[test]
fn test_fee_rate_statics() {
    let mut provider = DummyFeeRateProvider::new();
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
        Some(FeeRateStatics {
            mean: 11.0,
            median: 11.0
        })
    );

    let statistics = FeeRateCollector::new(&provider).statistics(Some(9));
    assert_eq!(
        statistics,
        Some(FeeRateStatics {
            mean: 17.0,
            median: 17.0
        })
    );

    let statistics = FeeRateCollector::new(&provider).statistics(Some(30));
    assert_eq!(
        statistics,
        Some(FeeRateStatics {
            mean: 11.0,
            median: 11.0
        })
    );

    let statistics = FeeRateCollector::new(&provider).statistics(Some(0));
    assert_eq!(
        statistics,
        Some(FeeRateStatics {
            mean: 21.0,
            median: 21.0
        })
    );
}
