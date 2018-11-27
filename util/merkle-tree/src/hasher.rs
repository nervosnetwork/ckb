use hash::Sha3;

pub trait Hasher {
    type Item;
    fn hash(&self, node1: &Self::Item, node2: &Self::Item) -> Self::Item;
}

pub struct DefaultHasher;

impl Hasher for DefaultHasher {
    type Item = [u8; 32];

    fn hash(&self, node1: &Self::Item, node2: &Self::Item) -> Self::Item {
        let mut hash = [0u8; 32];
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(node1);
        sha3.update(node2);
        sha3.finalize(&mut hash);
        hash
    }
}
