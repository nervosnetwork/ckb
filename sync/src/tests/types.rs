use crate::types::{FILTER_TTL, TtlFilter};

// test_get_ancestor removed - get_ancestor functionality moved to ActiveChain
// and no longer uses skip list optimization

#[test]
fn ttl_filter() {
    let mut filter = TtlFilter::default();
    let mut _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);
    filter.insert(1);
    let mut _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(FILTER_TTL * 1000 + 1000);
    filter.insert(2);
    filter.remove_expired();
    assert!(!filter.contains(&1));
    assert!(filter.contains(&2));
}
