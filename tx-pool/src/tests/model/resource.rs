#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PayloadBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResolvedResidentBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EntryMetadataBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EdgeMetadataBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct TotalRetainedBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetainedChargeInputs {
    pub(super) payload: PayloadBytes,
    pub(super) resolved: ResolvedResidentBytes,
    pub(super) entry_metadata: EntryMetadataBytes,
    pub(super) edge_metadata: EdgeMetadataBytes,
}

impl RetainedChargeInputs {
    pub(super) fn compile(self) -> Option<TotalRetainedBytes> {
        self.payload
            .0
            .max(self.resolved.0)
            .checked_add(self.entry_metadata.0)?
            .checked_add(self.edge_metadata.0)
            .map(TotalRetainedBytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeGrant {
    pub(super) max_total_retained: TotalRetainedBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComputeAdmission {
    Granted(TotalRetainedBytes),
    ResourceExcluded,
    ArithmeticExcluded,
}

impl ComputeGrant {
    pub(super) fn admit(self, inputs: RetainedChargeInputs) -> ComputeAdmission {
        let Some(total) = inputs.compile() else {
            return ComputeAdmission::ArithmeticExcluded;
        };
        if total <= self.max_total_retained {
            ComputeAdmission::Granted(total)
        } else {
            ComputeAdmission::ResourceExcluded
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScratchDisposition {
    Prepared,
    OrdinaryUnavailable,
}

pub(super) fn prepare_bounded_scratch(
    requested_items: u16,
    item_bound: u16,
    allocation_available: bool,
) -> ScratchDisposition {
    if requested_items > item_bound || !allocation_available {
        ScratchDisposition::OrdinaryUnavailable
    } else {
        ScratchDisposition::Prepared
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryCostInputs {
    pub(super) concurrent_queries: u32,
    pub(super) owner_rows: u32,
    pub(super) accepted_order_rows: u32,
    pub(super) output_items: u32,
    pub(super) output_item_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryCostUpperBound {
    pub(super) authority_row_visits: u64,
    pub(super) sort_comparisons: u64,
    pub(super) output_resident_bytes: u64,
}

impl QueryCostInputs {
    /// Compile the static adversarial upper-bound terms that remain visible
    /// even before profiling. `sort_comparisons` uses `n * ceil(log2(n))`;
    /// it is a comparison-count bound, not a wall-time prediction.
    pub(super) fn compile(self) -> Option<QueryCostUpperBound> {
        let concurrency = u64::from(self.concurrent_queries);
        let owners = u64::from(self.owner_rows);
        let accepted = u64::from(self.accepted_order_rows);
        let authority_row_visits = concurrency.checked_mul(owners.checked_add(accepted)?)?;
        let sort_levels = if self.accepted_order_rows <= 1 {
            0
        } else {
            u64::from(u32::BITS - (self.accepted_order_rows - 1).leading_zeros())
        };
        let sort_comparisons = concurrency
            .checked_mul(accepted)?
            .checked_mul(sort_levels)?;
        let output_resident_bytes = concurrency
            .checked_mul(u64::from(self.output_items))?
            .checked_mul(u64::from(self.output_item_bytes))?;
        Some(QueryCostUpperBound {
            authority_row_visits,
            sort_comparisons,
            output_resident_bytes,
        })
    }
}
