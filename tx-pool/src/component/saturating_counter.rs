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

    /// Checked subtraction.
    fn checked_sub(self, rhs: Self) -> Option<Self>;
}

impl CounterValue for usize {
    fn zero() -> Self {
        0
    }

    fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.checked_sub(rhs)
    }
}

impl CounterValue for u64 {
    fn zero() -> Self {
        0
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
}
