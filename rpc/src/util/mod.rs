pub(crate) mod fee_rate;

pub(crate) use fee_rate::FeeRateCollector;

#[cfg(test)]
pub(crate) use fee_rate::FeeRateProvider;
