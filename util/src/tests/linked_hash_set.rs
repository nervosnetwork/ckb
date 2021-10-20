use ckb_fixed_hash::{h256, H256};

use crate::LinkedHashSet;

#[test]
fn insertion_order() {
    let tx1 = h256!("0x3b6b6ee76e80d1662911130194db2f962a28d30bd574fa792f78debaa8e3a385");
    let tx2 = h256!("0xbd15c6158328c1dfa7eaf8eec395282844d3c436c5db25bd644dd1436608fe69");
    let tx3 = h256!("0x544e23972f2b400aa8d4147240bd30d46eb0cfe8cdb436b2c8e827a4033a1c03");
    let tx4 = h256!("0xa9cc641af5fa07606c98bba6a5774379b5ba3985a2047852cf2cb946d3387b61");
    let tx5 = h256!("0x47f40d1839c3fb56bf269605593337b2dc7db1c395b30bb9568e4274df71ea24");
    let tx6 = h256!("0x1df1e5f580c6c10b858960504f14fca4d178cbb54425d021cb2361de1079b174");

    let txs = vec![tx1, tx2, tx3, tx4, tx5, tx6];

    let mut set = LinkedHashSet::default();
    set.extend(txs.iter().cloned());
    let diff: Vec<H256> = set.difference(&LinkedHashSet::default()).cloned().collect();
    assert!(txs == diff);
}
