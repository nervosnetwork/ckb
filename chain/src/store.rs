use bigint::H256;
use core::block::{Block, Header};
use db::kvdb::KeyValueDB;

const HEAD_HEADER_KEY: u8 = 'H' as u8;

pub trait ChainStore {
    fn get_block(&self, h: &H256) -> Option<Block>;
    fn save_block(&self, b: &Block);
    fn get_header(&self, h: &H256) -> Option<Header>;
    fn save_header(&self, h: &Header);
    fn head_header(&self) -> Option<Header>;
    fn init(&self, genesis: &Block) -> ();
}

pub struct ChainKVStore<T: KeyValueDB> {
    pub db: Box<T>,
}

impl<T: KeyValueDB> ChainStore for ChainKVStore<T> {
    // TODO error log
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.db.read(h).ok().unwrap()
    }

    fn save_block(&self, b: &Block) {
        self.db.write(&b.hash(), b).unwrap();
    }

    fn get_header(&self, h: &H256) -> Option<Header> {
        self.db.read(h).ok().unwrap()
    }

    fn save_header(&self, h: &Header) {
        self.db.write(&h.hash(), h).unwrap();
    }

    fn head_header(&self) -> Option<Header> {
        self.db.read(&vec![HEAD_HEADER_KEY]).ok().unwrap()
    }

    fn init(&self, genesis: &Block) {
        self.save_block(genesis);
        self.save_header(&genesis.header);
        self.db.write(&vec![HEAD_HEADER_KEY], &genesis.header).unwrap();
        ()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::kvdb::MemoryKeyValueDB;
    use genesis::genesis_dev;

    #[test]
    fn save_and_get_block() {
        let db = MemoryKeyValueDB::default();
        let store = ChainKVStore { db: Box::new(db) };
        let block = genesis_dev();
        let hash = block.hash();
        store.save_block(&block);
        assert_eq!(block, store.get_block(&hash).unwrap());
    }
}
