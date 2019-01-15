use crate::flat_serializer::serialized_addresses;
use crate::store::{ChainKVStore, ChainStore, ChainTip, META_TIP_HASH_KEY};
use crate::{COLUMN_BLOCK_BODY, COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_ADDR};
use bincode::{deserialize, serialize};
use ckb_core::block::Block;
use ckb_core::extras::{BlockExt, TransactionAddress};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Transaction, TransactionBuilder};
use ckb_db::batch::Batch;
use ckb_db::kvdb::KeyValueDB;
use numext_fixed_hash::H256;

// maintain chain index, extend chainstore
pub trait ChainIndex: ChainStore {
    fn init(&self, genesis: &Block);

    fn get_block_hash(&self, number: BlockNumber) -> Option<H256>;
    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber>;
    fn get_tip_header(&self) -> Option<Header>;
    fn get_transaction(&self, h: &H256) -> Option<Transaction>;
    fn get_transaction_address(&self, hash: &H256) -> Option<TransactionAddress>;

    fn rollback(&self);
    fn forward(&self, hash: &H256);
}

impl<T: 'static + KeyValueDB> ChainIndex for ChainKVStore<T> {
    fn init(&self, genesis: &Block) {
        let genesis_hash = genesis.header().hash();
        let block_ext = BlockExt {
            received_at: genesis.header().timestamp(),
            total_difficulty: genesis.header().difficulty().clone(),
            total_uncles_count: 0,
        };
        self.save_with_batch(|batch| {
            self.insert_block(batch, genesis);
            self.insert_block_ext(batch, &genesis_hash, &block_ext);
            Ok(())
        })
        .expect("genesis init");

        let new_tip = ChainTip {
            number: 0,
            hash: genesis_hash,
            total_difficulty: block_ext.total_difficulty,
        };
        self.update_tip(new_tip);
    }

    fn get_block_hash(&self, number: BlockNumber) -> Option<H256> {
        let key = serialize(&number).unwrap();
        self.get(COLUMN_INDEX, &key)
            .map(|raw| H256::from_slice(&raw[..]).expect("db safe access"))
    }

    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.get(COLUMN_INDEX, hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_tip_header(&self) -> Option<Header> {
        self.get_header(&self.get_tip().read().hash)
    }

    fn get_transaction(&self, h: &H256) -> Option<Transaction> {
        self.get_transaction_address(h)
            .and_then(|d| {
                self.partial_get(
                    COLUMN_BLOCK_BODY,
                    d.block_hash.as_bytes(),
                    &(d.offset..(d.offset + d.length)),
                )
            })
            .map(|ref serialized_transaction| {
                TransactionBuilder::new(serialized_transaction).build()
            })
    }

    fn get_transaction_address(&self, h: &H256) -> Option<TransactionAddress> {
        self.get(COLUMN_TRANSACTION_ADDR, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    /// Rollback current tip.
    fn rollback(&self) {
        let mut chain_tip = self.tip.write();
        let header = self
            .get_header(&chain_tip.hash)
            .expect("inconsistent store");
        let transactions = self
            .get_block_body(&chain_tip.hash)
            .expect("inconsistent store");;

        let new_tip = ChainTip {
            hash: header.parent_hash().clone(),
            number: header.number() - 1,
            total_difficulty: chain_tip.total_difficulty.clone() - header.difficulty(),
        };

        self.save_with_batch(|batch| {
            batch.delete(COLUMN_INDEX, serialize(&chain_tip.number).unwrap());
            batch.delete(COLUMN_INDEX, chain_tip.hash.to_vec());
            transactions
                .iter()
                .for_each(|tx| batch.delete(COLUMN_TRANSACTION_ADDR, tx.hash().to_vec()));
            batch.insert(
                COLUMN_META,
                META_TIP_HASH_KEY.to_vec(),
                new_tip.hash.to_vec(),
            );
            Ok(())
        });

        *chain_tip = new_tip;
    }

    /// Forward to a new tip, assumes that parent block is current tip.
    fn forward(&self, hash: &H256) {
        let block_ext = self.get_block_ext(hash).expect("inconsistent store");
        let new_tip = ChainTip {
            number: self.tip.read().number + 1,
            hash: hash.clone(),
            total_difficulty: block_ext.total_difficulty,
        };
        self.update_tip(new_tip);
    }
}

impl<T: 'static + KeyValueDB> ChainKVStore<T> {
    fn update_tip(&self, new_tip: ChainTip) {
        let transactions = self
            .get_block_body(&new_tip.hash)
            .expect("inconsistent store");

        let mut chain_tip = self.tip.write();

        self.save_with_batch(|batch| {
            batch.insert(
                COLUMN_INDEX,
                serialize(&new_tip.number).unwrap(),
                new_tip.hash.to_vec(),
            );
            batch.insert(
                COLUMN_INDEX,
                new_tip.hash.to_vec(),
                serialize(&new_tip.number).unwrap(),
            );
            let addresses = serialized_addresses(transactions.iter()).unwrap();
            transactions.iter().enumerate().for_each(|(index, tx)| {
                let address = TransactionAddress {
                    block_hash: new_tip.hash.clone(),
                    offset: addresses[index].offset,
                    length: addresses[index].length,
                };
                batch.insert(
                    COLUMN_TRANSACTION_ADDR,
                    tx.hash().to_vec(),
                    serialize(&address).unwrap(),
                );
            });

            // let mut transaction_metas = HashMap::new();
            // TODO update transaction metas
            Ok(())
        });

        *chain_tip = new_tip;
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_db::diskdb::RocksDB;
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
        assert_eq!(&hash, &store.get_block_hash(0).unwrap());

        assert_eq!(
            block.header().difficulty(),
            &store.get_block_ext(&hash).unwrap().total_difficulty
        );

        assert_eq!(
            block.header().number(),
            store.get_block_number(&hash).unwrap()
        );

        assert_eq!(block.header(), &store.get_tip_header().unwrap());
    }
}
