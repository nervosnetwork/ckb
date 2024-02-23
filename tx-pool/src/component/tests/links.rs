use crate::component::links::{Relation, TxLinks, TxLinksMap};
use ckb_types::packed::ProposalShortId;
use ckb_types::prelude::Entity;
use std::collections::HashSet;

#[test]
fn test_link_map() {
    let mut map = TxLinksMap::default();
    let id1 = ProposalShortId::from_slice(&[1; 10]).unwrap();
    let id2 = ProposalShortId::from_slice(&[2; 10]).unwrap();
    let id3 = ProposalShortId::from_slice(&[3; 10]).unwrap();
    let id4 = ProposalShortId::from_slice(&[4; 10]).unwrap();

    map.add_link(id1.clone(), TxLinks::default());
    map.add_link(id2.clone(), TxLinks::default());
    map.add_link(id3.clone(), TxLinks::default());
    map.add_link(id4.clone(), TxLinks::default());

    map.add_parent(&id1, id2.clone());
    let expect: HashSet<ProposalShortId> = vec![id2.clone()].into_iter().collect();
    assert_eq!(map.get_parents(&id1).unwrap(), &expect);

    map.add_parent(&id1, id2.clone());
    map.add_parent(&id2, id3.clone());
    map.add_parent(&id3, id4.clone());
    let parents = map.calc_relation_ids([id1.clone()].into(), Relation::Parents);
    assert_eq!(parents.len(), 4);

    map.remove(&id3);
    map.remove_parent(&id2, &id3);
    let parents = map.calc_relation_ids([id1.clone()].into(), Relation::Parents);
    assert_eq!(parents.len(), 2);
}
