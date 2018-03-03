use bigint::H256;
use core::block::{Block, Header};
use db::kvdb::KeyValueDB;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum Key {
    BlockHash(u64),
    BlockHeader(H256),
    Block(H256),
    Transaction(H256),
    HeadHeader,
}

pub trait ChainStore: Sync + Send {
    fn get_block(&self, h: &H256) -> Option<Block>;
    fn save_block(&self, b: &Block);
    fn get_header(&self, h: &H256) -> Option<Header>;
    fn save_header(&self, h: &Header);
    fn get_block_hash(&self, height: u64) -> Option<H256>;
    fn save_block_hash(&self, height: u64, hash: &H256);
    fn head_header(&self) -> Option<Header>;
    fn save_head_header(&self, h: &Header);
    fn init(&self, genesis: &Block) -> ();
}

pub struct ChainKVStore<T: KeyValueDB> {
    pub db: Box<T>,
}

impl<T: KeyValueDB> ChainStore for ChainKVStore<T> {
    // TODO error log
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.db.read(&Key::Block(*h)).ok().unwrap()
    }

    fn save_block(&self, b: &Block) {
        self.db.write(&Key::Block(b.hash()), b).unwrap();
    }

    fn get_header(&self, h: &H256) -> Option<Header> {
        self.db.read(&Key::BlockHeader(*h)).ok().unwrap()
    }

    fn save_header(&self, h: &Header) {
        self.db.write(&Key::BlockHeader(h.hash()), h).unwrap();
    }

    fn save_block_hash(&self, height: u64, hash: &H256) {
        self.db.write(&Key::BlockHash(height), hash).unwrap();
    }

    fn get_block_hash(&self, height: u64) -> Option<H256> {
        self.db.read(&Key::BlockHash(height)).unwrap()
    }

    fn head_header(&self) -> Option<Header> {
        self.db.read(&Key::HeadHeader).ok().unwrap()
    }

    fn save_head_header(&self, h: &Header) {
        self.db.write(&Key::HeadHeader, h).unwrap();
    }

    fn init(&self, genesis: &Block) {
        self.save_block(genesis);
        self.save_header(&genesis.header);
        self.save_head_header(&genesis.header);
        self.save_block_hash(genesis.header.height, &genesis.hash());
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
