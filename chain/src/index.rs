use super::flat_serializer::serialized_addresses;
use bigint::H256;
use bincode::{deserialize, serialize};
use core::block::Block;
use core::extras::{BlockExt, TransactionAddress};
use core::header::{BlockNumber, Header};
use core::transaction::{Transaction, TransactionBuilder};
use db::batch::Batch;
use db::kvdb::KeyValueDB;
use error::Error;
use store::{ChainKVStore, ChainStore};
use {COLUMN_BLOCK_BODY, COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_ADDR};

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";

// maintain chain index, extend chainstore
pub trait ChainIndex: ChainStore {
    fn init(&self, genesis: &Block);
    fn get_block_hash(&self, number: BlockNumber) -> Option<H256>;
    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber>;
    fn get_tip_header(&self) -> Option<Header>;
    fn get_transaction(&self, h: &H256) -> Option<Transaction>;
    fn get_transaction_address(&self, hash: &H256) -> Option<TransactionAddress>;

    fn insert_block_hash(&self, batch: &mut Batch, number: BlockNumber, hash: &H256);
    fn delete_block_hash(&self, batch: &mut Batch, number: BlockNumber);
    fn insert_block_number(&self, batch: &mut Batch, hash: &H256, number: BlockNumber);
    fn delete_block_number(&self, batch: &mut Batch, hash: &H256);
    fn insert_tip_header(&self, batch: &mut Batch, h: &Header);
    fn insert_transaction_address(&self, batch: &mut Batch, block_hash: &H256, txs: &[Transaction]);
    fn delete_transaction_address(&self, batch: &mut Batch, txs: &[Transaction]);
}

impl<T: 'static + KeyValueDB> ChainIndex for ChainKVStore<T> {
    fn init(&self, genesis: &Block) {
        self.save_with_batch(|batch| {
            let genesis_hash = genesis.header().hash();
            let ext = BlockExt {
                received_at: genesis.header().timestamp(),
                total_difficulty: genesis.header().difficulty(),
                total_uncles_count: 0,
            };

            let mut cells = Vec::with_capacity(genesis.commit_transactions().len());

            for tx in genesis.commit_transactions() {
                let ins = if tx.is_cellbase() {
                    Vec::new()
                } else {
                    tx.input_pts()
                };
                let outs = tx.output_pts();

                cells.push((ins, outs));
            }

            let output_root = self
                .update_transaction_meta(batch, H256::zero(), cells)
                .ok_or(Error::InvalidOutput)?;
            self.insert_block(batch, genesis);
            self.insert_block_ext(batch, &genesis_hash, &ext);
            self.insert_tip_header(batch, &genesis.header());
            self.insert_output_root(batch, genesis_hash, output_root);
            self.insert_block_hash(batch, 0, &genesis_hash);
            self.insert_block_number(batch, &genesis_hash, 0);
            self.insert_transaction_address(batch, &genesis_hash, genesis.commit_transactions());
            Ok(())
        }).expect("genesis init");
    }

    fn get_block_hash(&self, number: BlockNumber) -> Option<H256> {
        let key = serialize(&number).unwrap();
        self.get(COLUMN_INDEX, &key).map(|raw| H256::from(&raw[..]))
    }

    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.get(COLUMN_INDEX, &hash)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_tip_header(&self) -> Option<Header> {
        self.get(COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|raw| self.get_header(&H256::from(&raw[..])))
            .map(Into::into)
    }

    fn get_transaction(&self, h: &H256) -> Option<Transaction> {
        self.get_transaction_address(h)
            .and_then(|d| {
                self.partial_get(
                    COLUMN_BLOCK_BODY,
                    &d.block_hash,
                    &(d.offset..(d.offset + d.length)),
                )
            }).map(|ref serialized_transaction| {
                TransactionBuilder::new(serialized_transaction).with_hash(h)
            })
    }

    fn get_transaction_address(&self, h: &H256) -> Option<TransactionAddress> {
        self.get(COLUMN_TRANSACTION_ADDR, &h)
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn insert_tip_header(&self, batch: &mut Batch, h: &Header) {
        batch.insert(COLUMN_META, META_TIP_HEADER_KEY.to_vec(), h.hash().to_vec());
    }

    fn insert_block_hash(&self, batch: &mut Batch, number: BlockNumber, hash: &H256) {
        let key = serialize(&number).unwrap();
        batch.insert(COLUMN_INDEX, key, hash.to_vec());
    }

    fn insert_block_number(&self, batch: &mut Batch, hash: &H256, number: BlockNumber) {
        batch.insert(COLUMN_INDEX, hash.to_vec(), serialize(&number).unwrap());
    }

    fn insert_transaction_address(
        &self,
        batch: &mut Batch,
        block_hash: &H256,
        txs: &[Transaction],
    ) {
        let addresses = serialized_addresses(txs.iter()).unwrap();
        for (id, tx) in txs.iter().enumerate() {
            let address = TransactionAddress {
                block_hash: *block_hash,
                offset: addresses[id].offset,
                length: addresses[id].length,
            };
            batch.insert(
                COLUMN_TRANSACTION_ADDR,
                tx.hash().to_vec(),
                serialize(&address).unwrap(),
            );
        }
    }

    fn delete_transaction_address(&self, batch: &mut Batch, txs: &[Transaction]) {
        for tx in txs {
            batch.delete(COLUMN_TRANSACTION_ADDR, tx.hash().to_vec());
        }
    }

    fn delete_block_hash(&self, batch: &mut Batch, number: BlockNumber) {
        let key = serialize(&number).unwrap();
        batch.delete(COLUMN_INDEX, key);
    }

    fn delete_block_number(&self, batch: &mut Batch, hash: &H256) {
        batch.delete(COLUMN_INDEX, hash.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use consensus::Consensus;
    use db::diskdb::RocksDB;
    use tempfile;

    #[test]
    fn index_store() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("index_init")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, COLUMNS);
        let store = ChainKVStore::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();
        let hash = block.header().hash();
        store.init(&block);
        assert_eq!(hash, store.get_block_hash(0).unwrap());

        assert_eq!(
            block.header().difficulty(),
            store.get_block_ext(&hash).unwrap().total_difficulty
        );

        assert_eq!(
            block.header().number(),
            store.get_block_number(&hash).unwrap()
        );

        assert_eq!(*block.header(), store.get_tip_header().unwrap());
    }
}
