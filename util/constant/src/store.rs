/// This value is try to set tx key range as tight as possible,
/// so that db iterating can stop sooner, rather than walking over the whole range of tombstones.
/// empty_tx_size = 72
pub const TX_INDEX_UPPER_BOUND: u32 = 597 * 1000 / 72;
