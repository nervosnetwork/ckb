// time interval between two mining tries(ms)
pub const TIME_STEP: u64 = 250;

// parameters used for calculating difficulty
pub const INCREMENT_DIVISOR: u64 = 15_000;
pub const THRESHOLD: u64 = 1;
pub const DIFFICULTY_BOUND_DIVISOR: u64 = 2048;
pub const LIMIT: u64 = 99;

// parameters used for calculating block height
// suppose EPOCH_LEN = 10, HEIGHT_SHIFT = 50,
// then when my height is between 60-69, use height 10 block as the challenge
// when my height is 100-109, use height 50 block as the challenge.
pub const EPOCH_LEN: u64 = 10;
pub const HEIGHT_SHIFT: u64 = 50;
