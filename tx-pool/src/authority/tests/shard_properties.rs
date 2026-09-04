use super::super::plan::TxPoolAuthority;
use super::super::state::{AcceptedStatus, RawTxHash};
use super::foundation::{accept_remote_transaction, limits, tx};

#[test]
fn coherent_read_cut_routes_every_point_lookup_to_the_same_owner_as_a_full_scan() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let owners = (50_000u64..50_192)
        .map(|seed| {
            accept_remote_transaction(
                &mut authority,
                tx(seed),
                seed as usize,
                AcceptedStatus::Pending,
                Vec::new(),
            )
        })
        .collect::<Vec<_>>();
    let absent = (60_000u64..60_064)
        .map(|seed| RawTxHash(tx(seed).hash()))
        .collect::<Vec<_>>();

    let cut = authority.entries_for_reference().read_all();
    for key in owners.iter().chain(&absent) {
        let routed = cut.get(key);
        let scanned = cut
            .iter()
            .find_map(|(stored, owner)| (stored == key).then_some(owner));
        assert_eq!(
            routed.is_some(),
            scanned.is_some(),
            "owner presence differs"
        );
        if let (Some(routed), Some(scanned)) = (routed, scanned) {
            assert!(
                std::ptr::eq(routed, scanned),
                "routed and scanned lookups must borrow the same owner"
            );
        }
    }
}
