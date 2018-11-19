// time interval between two mining tries(ms)
// pub const TIME_STEP: u64 = 250;

// max time deviation
// pub const MAX_TIME_DEVIAT: u64 = 30_000;

// parameters used for calculating difficulty
pub const INCREMENT_DIVISOR: u64 = 9_000;
pub const THRESHOLD: u64 = 1;
pub const DIFFICULTY_BOUND_DIVISOR: u64 = 0x800;
pub const LIMIT: u64 = 99;

// parameters used for calculating block number
// suppose EPOCH_LEN = 10, HEIGHT_SHIFT = 50,
// then when my number is between 60-69, use number 10 block as the challenge
// when my number is 100-109, use number 50 block as the challenge.
// pub const EPOCH_LEN: u64 = 10;
// pub const HEIGHT_SHIFT: u64 = 50;

//Min difficulty
pub const MIN_DIFFICULTY: u64 = 0x2_0000;
