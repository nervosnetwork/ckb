//! Keep each benchmark submission within the controller's relay envelope.

use ckb_constant::sync::{MAX_RELAY_TXS_BYTES_PER_BATCH, MAX_RELAY_TXS_NUM_PER_BATCH};

pub(crate) fn batch_len(sizes: impl Iterator<Item = usize>) -> Result<usize, &'static str> {
    let mut bytes = 0;
    let mut count = 0;
    for size in sizes.take(MAX_RELAY_TXS_NUM_PER_BATCH) {
        if size > MAX_RELAY_TXS_BYTES_PER_BATCH {
            return Err("benchmark transaction exceeds the relay batch byte limit");
        }
        if size > MAX_RELAY_TXS_BYTES_PER_BATCH - bytes {
            break;
        }
        bytes += size;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_input_and_oversized_transaction() {
        use super::{MAX_RELAY_TXS_BYTES_PER_BATCH, batch_len};
        assert_eq!(batch_len(std::iter::empty()), Ok(0));
        assert!(batch_len([MAX_RELAY_TXS_BYTES_PER_BATCH + 1].into_iter()).is_err());
        assert!(batch_len([usize::MAX].into_iter()).is_err());
    }

    #[test]
    fn exact_byte_limit_and_spill_preserve_the_next_item() {
        use super::{MAX_RELAY_TXS_BYTES_PER_BATCH, batch_len};
        let sizes = [MAX_RELAY_TXS_BYTES_PER_BATCH - 1, 1, 1];
        let first = batch_len(sizes.iter().copied()).unwrap();
        assert_eq!(first, 2);
        assert_eq!(batch_len(sizes[first..].iter().copied()), Ok(1));
        assert_eq!(
            batch_len([MAX_RELAY_TXS_BYTES_PER_BATCH].into_iter()),
            Ok(1)
        );
    }

    #[test]
    fn count_limit_applies_before_the_byte_limit() {
        use super::{MAX_RELAY_TXS_NUM_PER_BATCH, batch_len};
        assert_eq!(
            batch_len(std::iter::repeat_n(1, MAX_RELAY_TXS_NUM_PER_BATCH)),
            Ok(MAX_RELAY_TXS_NUM_PER_BATCH)
        );
        assert_eq!(
            batch_len(std::iter::repeat_n(1, MAX_RELAY_TXS_NUM_PER_BATCH + 1)),
            Ok(MAX_RELAY_TXS_NUM_PER_BATCH)
        );
    }
}
