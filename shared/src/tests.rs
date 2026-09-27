use crate::{HeaderIndex, SharedBuilder};
use ckb_types::{U256, core::HeaderBuilder, packed::Byte32};

#[test]
fn unverified_tip_uses_the_verified_snapshot_without_rewriting_the_candidate() {
    let (shared, _package) = SharedBuilder::with_temp_db().build().unwrap();
    let before = shared.cloned_snapshot();
    let verified = HeaderBuilder::default()
        .number(10)
        .epoch(before.epoch_ext().number_with_fraction(10))
        .build();
    let total_difficulty = U256::from(1234u64);
    shared.store_snapshot(shared.new_snapshot(
        verified.clone(),
        total_difficulty.clone(),
        before.epoch_ext().clone(),
        before.proposals().clone(),
    ));

    let candidate = HeaderIndex::new(9, Byte32::zero(), U256::zero());
    shared.set_unverified_tip(candidate.clone());
    assert_eq!(
        shared.get_unverified_tip(),
        HeaderIndex::new(10, verified.hash(), total_difficulty)
    );
    // A read must not write an old observation over a concurrent publication.
    assert_eq!(shared.unverified_tip.load().as_ref(), &candidate);
}

#[test]
fn unverified_tip_preserves_equal_and_higher_candidates() {
    let (shared, _package) = SharedBuilder::with_temp_db().build().unwrap();
    let tip = shared.snapshot().tip_number();
    // An equal-height candidate may belong to another fork. Its identity is
    // still the producer's observation, rather than the canonical tip's hash.
    for number in [tip, tip + 1] {
        let candidate = HeaderIndex::new(number, Byte32::zero(), U256::from(42u64));
        shared.set_unverified_tip(candidate.clone());
        assert_eq!(shared.get_unverified_tip(), candidate);
    }
}
