use crate::store::SQLXPool;
use crate::AsyncIndexerRHandle;

use ckb_indexer_sync::Error;
use ckb_jsonrpc_types::IndexerTip;
use ckb_types::H256;
use sqlx::Row;

impl AsyncIndexerRHandle {
    /// Get indexer current tip
    pub async fn get_indexer_tip(&self) -> Result<Option<IndexerTip>, Error> {
        let query = SQLXPool::new_query(
            r#"
            SELECT block_number block_hash FROM block
            ORDER BY block_number
            DESC LIMIT 1
            "#,
        );
        self.store
            .fetch_optional(query)
            .await
            .map(|res| {
                res.map(|row| IndexerTip {
                    block_number: (row.get::<i32, _>("block_number") as u64).into(),
                    block_hash: bytes_to_h256(row.get("block_hash")),
                })
            })
            .map_err(|err| Error::DB(err.to_string()))
    }
}

pub(crate) fn bytes_to_h256(input: &[u8]) -> H256 {
    H256::from_slice(&input[0..32]).expect("bytes to h256")
}
