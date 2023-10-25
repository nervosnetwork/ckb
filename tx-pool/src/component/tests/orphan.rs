use crate::component::orphan::OrphanPool;
use crate::component::tests::util::build_tx;
use ckb_types::packed::Byte32;

#[test]
fn test_orphan() {
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
    let mut orphan = OrphanPool::new();
    assert_eq!(orphan.len(), 0);
    assert!(!orphan.contains_key(&tx1.proposal_short_id()));

    orphan.add_orphan_tx(tx1.clone(), 0.into(), 0);
    assert_eq!(orphan.len(), 1);

    orphan.add_orphan_tx(tx1.clone(), 0.into(), 0);
    assert_eq!(orphan.len(), 1);

    let tx2 = build_tx(vec![(&tx1.hash(), 0)], 1);
    orphan.add_orphan_tx(tx2.clone(), 0.into(), 0);
    assert_eq!(orphan.len(), 2);

    orphan.remove_orphan_tx(&tx1.proposal_short_id());
    assert_eq!(orphan.len(), 1);
    orphan.remove_orphan_tx(&tx2.proposal_short_id());
    assert_eq!(orphan.len(), 0);
}

#[test]
fn test_orphan_duplicated() {
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 3);
    let mut orphan = OrphanPool::new();

    let tx2 = build_tx(vec![(&tx1.hash(), 0)], 1);
    let tx3 = build_tx(vec![(&tx2.hash(), 0)], 1);
    let tx4 = build_tx(vec![(&tx3.hash(), 0), (&tx1.hash(), 1)], 1);
    let tx5 = build_tx(vec![(&tx1.hash(), 0)], 2);
    orphan.add_orphan_tx(tx1.clone(), 0.into(), 0);
    orphan.add_orphan_tx(tx2.clone(), 0.into(), 0);
    orphan.add_orphan_tx(tx3.clone(), 0.into(), 0);
    orphan.add_orphan_tx(tx4.clone(), 0.into(), 0);
    orphan.add_orphan_tx(tx5.clone(), 0.into(), 0);
    assert_eq!(orphan.len(), 5);

    let txs = orphan.find_by_previous(&tx2);
    assert_eq!(txs.len(), 1);

    let txs = orphan.find_by_previous(&tx1);
    assert_eq!(txs.len(), 3);
    assert!(txs.contains(&tx2.proposal_short_id()));
    assert!(txs.contains(&tx4.proposal_short_id()));
    assert!(txs.contains(&tx5.proposal_short_id()));

    orphan.remove_orphan_tx(&tx4.proposal_short_id());
    let txs = orphan.find_by_previous(&tx1);
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&tx2.proposal_short_id()));
    assert!(txs.contains(&tx5.proposal_short_id()));
}
