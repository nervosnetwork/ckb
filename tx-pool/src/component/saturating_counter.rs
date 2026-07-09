//! Saturating counters with underflow recovery.

use ckb_logger::error;
use std::fmt::Display;

/// A counter that detects underflow/overflow and can recover by recomputing the
/// true value.
///
/// Used for cached `total_tx_size` / `total_tx_cycles` values that can become
/// inconsistent with the underlying collection after concurrent or partial
/// removals.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SaturatingCounter<T> {
    value: T,
}

/// Value types supported by [`SaturatingCounter`].
pub(crate) trait CounterValue: Copy + Default + Display + PartialOrd {
    /// The additive identity.
    fn zero() -> Self;

    /// The maximum representable value.
    fn max_value() -> Self;

    /// Checked addition.
    fn checked_add(self, rhs: Self) -> Option<Self>;

    /// Checked subtraction.
    fn checked_sub(self, rhs: Self) -> Option<Self>;
}

impl CounterValue for usize {
    fn zero() -> Self {
        0
    }

    fn max_value() -> Self {
        Self::MAX
    }

    fn checked_add(self, rhs: Self) -> Option<Self> {
        self.checked_add(rhs)
    }

    fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.checked_sub(rhs)
    }
}

impl CounterValue for u64 {
    fn zero() -> Self {
        0
    }

    fn max_value() -> Self {
        Self::MAX
    }

    fn checked_add(self, rhs: Self) -> Option<Self> {
        self.checked_add(rhs)
    }

    fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.checked_sub(rhs)
    }
}

impl<T: CounterValue> SaturatingCounter<T> {
    /// Create a counter with the given initial value.
    pub(crate) fn new(value: T) -> Self {
        Self { value }
    }

    /// Current value.
    pub(crate) fn get(&self) -> T {
        self.value
    }

    /// Set the value directly.
    pub(crate) fn set(&mut self, value: T) {
        self.value = value;
    }

    /// Subtract `delta`. If the subtraction would underflow, use `recompute`
    /// (precomputed by the caller) as the recovered value. If `recompute` is
    /// `None`, keep the current value.
    pub(crate) fn sub_or_recompute(
        &mut self,
        delta: T,
        recompute: Option<T>,
        name: &'static str,
        action: &'static str,
    ) {
        match self.value.checked_sub(delta) {
            Some(v) => self.value = v,
            None => match recompute {
                Some(v) => {
                    error!(
                        "{} {} underflowed by sub {} in {}, recomputed {}",
                        name, self.value, delta, action, v
                    );
                    self.value = v;
                }
                None => {
                    error!(
                        "{} {} underflowed by sub {} in {}, and recomputing overflowed",
                        name, self.value, delta, action
                    );
                }
            },
        }
    }

    /// Subtract `delta`. If the subtraction would underflow, reset to zero.
    pub(crate) fn sub_or_zero(&mut self, delta: T, name: &'static str, action: &'static str) {
        match self.value.checked_sub(delta) {
            Some(v) => self.value = v,
            None => {
                error!(
                    "{} {} underflowed by sub {} in {}, reset to zero",
                    name, self.value, delta, action
                );
                self.value = T::zero();
            }
        }
    }

    /// Add `delta` with saturation on overflow.
    ///
    /// If the addition would overflow the representable range, the counter is
    /// clamped to the maximum value and an error is logged. This matches the
    /// behaviour of the manual `get().saturating_add(...).set(...)` pattern it
    /// replaces.
    pub(crate) fn add_saturating(&mut self, delta: T, name: &'static str, action: &'static str) {
        match self.value.checked_add(delta) {
            Some(v) => self.value = v,
            None => {
                error!(
                    "{} {} overflowed by add {} in {}, clamped to max",
                    name, self.value, delta, action
                );
                self.value = T::max_value();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_saturating_normal() {
        let mut c = SaturatingCounter::<usize>::new(10);
        c.add_saturating(5, "test", "add");
        assert_eq!(c.get(), 15);
    }

    #[test]
    fn add_saturating_clamps_on_overflow() {
        let mut c = SaturatingCounter::<u64>::new(u64::MAX - 1);
        c.add_saturating(5, "test", "overflow");
        assert_eq!(c.get(), u64::MAX);
    }

    #[test]
    fn sub_or_zero_recovers_from_underflow() {
        let mut c = SaturatingCounter::<usize>::new(3);
        c.sub_or_zero(10, "test", "underflow");
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn sub_or_recompute_uses_provided_value() {
        let mut c = SaturatingCounter::<usize>::new(3);
        c.sub_or_recompute(10, Some(7), "test", "recompute");
        assert_eq!(c.get(), 7);
    }
}
