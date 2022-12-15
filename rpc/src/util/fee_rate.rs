use ckb_jsonrpc_types::FeeRateStatics;
use ckb_shared::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::{tx_pool::get_transaction_weight, BlockExt, BlockNumber, FeeRate};

const DEFAULT_TARGET: u64 = 21;
const MIN_TARGET: u64 = 1;
const MAX_TARGET: u64 = 101;

fn is_even(n: u64) -> bool {
    n & 1 == 0
}

fn mean(numbers: &[u64]) -> u64 {
    let sum: u64 = numbers.iter().sum();
    sum / numbers.len() as u64
}

fn median(numbers: &mut [u64]) -> u64 {
    numbers.sort_unstable();
    let mid = numbers.len() / 2;
    if numbers.len() % 2 == 0 {
        mean(&[numbers[mid - 1], numbers[mid]])
    } else {
        numbers[mid]
    }
}

pub(crate) trait FeeRateProvider {
    fn get_tip_number(&self) -> BlockNumber;
    fn get_block_ext_by_number(&self, number: BlockNumber) -> Option<BlockExt>;
    fn max_target(&self) -> u64;

    fn collect<F>(&self, target: u64, f: F) -> Vec<u64>
    where
        F: FnMut(Vec<u64>, BlockExt) -> Vec<u64>,
    {
        let tip_number = self.get_tip_number();
        let start = std::cmp::max(
            MIN_TARGET,
            tip_number.saturating_add(1).saturating_sub(target),
        );

        let block_ext_iter =
            (start..=tip_number).filter_map(|number| self.get_block_ext_by_number(number));
        block_ext_iter.fold(Vec::new(), f)
    }
}

impl FeeRateProvider for Snapshot {
    fn get_tip_number(&self) -> BlockNumber {
        self.tip_number()
    }

    fn get_block_ext_by_number(&self, number: BlockNumber) -> Option<BlockExt> {
        self.get_block_hash(number)
            .and_then(|hash| self.get_block_ext(&hash))
    }

    fn max_target(&self) -> u64 {
        MAX_TARGET
    }
}

// FeeRateCollector collect fee_rate related information
pub(crate) struct FeeRateCollector<'a, P> {
    provider: &'a P,
}

impl<'a, P> FeeRateCollector<'a, P>
where
    P: FeeRateProvider,
{
    pub fn new(provider: &'a P) -> Self {
        FeeRateCollector { provider }
    }

    pub fn statistics(&self, target: Option<u64>) -> Option<FeeRateStatics> {
        let mut target = target.unwrap_or(DEFAULT_TARGET);
        if is_even(target) {
            target = target.saturating_add(1);
        }
        target = std::cmp::min(self.provider.max_target(), target);

        let mut fee_rates = self.provider.collect(target, |mut fee_rates, block_ext| {
            if !block_ext.txs_fees.is_empty()
                && block_ext.cycles.is_some()
                && block_ext.txs_sizes.is_some()
            {
                for (fee, cycles, size) in itertools::izip!(
                    block_ext.txs_fees,
                    block_ext.cycles.expect("checked"),
                    block_ext.txs_sizes.expect("checked")
                ) {
                    let weight = get_transaction_weight(size as usize, cycles);
                    if weight > 0 {
                        fee_rates.push(FeeRate::calculate(fee, weight).as_u64());
                    }
                }
            }
            fee_rates
        });

        if fee_rates.is_empty() {
            None
        } else {
            Some(FeeRateStatics {
                mean: mean(&fee_rates).into(),
                median: median(&mut fee_rates).into(),
            })
        }
    }
}
