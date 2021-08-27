use crate::net_time_checker::{NetTimeChecker, TOLERANT_OFFSET};

#[test]
fn test_samples_collect() {
    let mut ntc = NetTimeChecker::new(3, 5, TOLERANT_OFFSET);
    // zero samples
    assert!(ntc.check().is_ok());
    // 1 sample
    ntc.add_sample(TOLERANT_OFFSET as i64 + 1);
    assert!(ntc.check().is_ok());
    // 3 samples
    ntc.add_sample(TOLERANT_OFFSET as i64 + 2);
    ntc.add_sample(TOLERANT_OFFSET as i64 + 3);
    assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 2);
    // 4 samples
    ntc.add_sample(1);
    assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 1);
    // 5 samples
    ntc.add_sample(2);
    assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 1);
    // 5 samples within tolerant offset
    ntc.add_sample(3);
    ntc.add_sample(4);
    ntc.add_sample(5);
    assert!(ntc.check().is_ok());
    // 5 samples negative offset
    ntc.add_sample(-(TOLERANT_OFFSET as i64) - 1);
    ntc.add_sample(-(TOLERANT_OFFSET as i64) - 2);
    assert!(ntc.check().is_ok());
    ntc.add_sample(-(TOLERANT_OFFSET as i64) - 3);
    assert_eq!(ntc.check().unwrap_err(), -(TOLERANT_OFFSET as i64) - 1);
}
