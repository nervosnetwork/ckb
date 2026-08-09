//! Pure relations for non-authoritative but contractual observations.
//!
//! These functions own no product state. They state the exact observations
//! that allocation, bounded cleanup, metrics and Rust ordering adapters must
//! expose without turning any observation into policy authority.

use std::cmp::Ordering;

pub(crate) const fn reservation_capacity_is_sufficient(
    requested: usize,
    observed_capacity: usize,
) -> bool {
    observed_capacity >= requested
}

pub(crate) const fn bounded_prefix_len(
    due: usize,
    caller_limit: usize,
    effect_limit: usize,
) -> usize {
    let selected = if due < caller_limit {
        due
    } else {
        caller_limit
    };
    if selected < effect_limit {
        selected
    } else {
        effect_limit
    }
}

pub(crate) const fn indexed_expiry_cut_is_current(
    owner_deadline: u8,
    indexed_deadline: u8,
    cutoff: u8,
) -> bool {
    owner_deadline == indexed_deadline && indexed_deadline <= cutoff
}

pub(crate) const fn accepted_expiry_fault(
    owner_deadline: u8,
    indexed_deadline: u8,
    cutoff: u8,
) -> bool {
    owner_deadline != indexed_deadline || owner_deadline > cutoff
}

pub(crate) const fn exact_operational_projection(
    kernel: [usize; 7],
    effects: [usize; 6],
) -> [usize; 13] {
    [
        kernel[0], kernel[1], kernel[2], kernel[3], kernel[4], kernel[5], kernel[6], effects[0],
        effects[1], effects[2], effects[3], effects[4], effects[5],
    ]
}

pub(crate) fn cumulative_effect_region_projection(
    remote: [usize; 2],
    trusted: [usize; 2],
    critical: [usize; 2],
) -> Option<[usize; 6]> {
    let ordinary = [
        remote[0].checked_add(trusted[0])?,
        remote[1].checked_add(trusted[1])?,
    ];
    let total = [
        ordinary[0].checked_add(critical[0])?,
        ordinary[1].checked_add(critical[1])?,
    ];
    Some([
        remote[0],
        remote[1],
        ordinary[0],
        ordinary[1],
        total[0],
        total[1],
    ])
}

pub(crate) fn total_partial_order_refines(total: Ordering, partial: Option<Ordering>) -> bool {
    partial == Some(total)
}
