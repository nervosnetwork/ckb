use crate::{core::TransactionMeta, packed::Byte32};

#[test]
fn set_unset_dead_out_of_bounds() {
    let mut meta = TransactionMeta::new(0, 0, Byte32::zero(), 4, false);
    meta.set_dead(3);
    assert!(meta.is_dead(3) == Some(true));
    meta.unset_dead(3);
    assert!(meta.is_dead(3) == Some(false));
    // none-op
    meta.set_dead(4);
    assert!(meta.is_dead(4) == None);
    meta.unset_dead(4);
    assert!(meta.is_dead(4) == None);
}
