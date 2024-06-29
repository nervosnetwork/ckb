/// The fee estimate mode.
#[derive(Clone, Copy, Debug)]
pub enum EstimateMode {
    /// No priority, expect the transaction to be committed in 1 hour.
    NoPriority,
    /// Low priority, expect the transaction to be committed in 30 minutes.
    LowPriority,
    /// Medium priority, expect the transaction to be committed in 10 minutes.
    MediumPriority,
    /// High priority, expect the transaction to be committed as soon as possible.
    HighPriority,
}

impl Default for EstimateMode {
    fn default() -> Self {
        Self::NoPriority
    }
}
