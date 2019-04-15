use faketime::unix_time_as_millis;
use jsonrpc_types::{Action, TxTrace};
use lru_cache::LruCache;
use numext_fixed_hash::H256;

macro_rules! define_method {
    ($name:ident, $action:expr) => {
        pub fn $name<S: ToString>(&mut self, hash: &H256, info: S) {
            self.inner.get_mut(hash).map(|v| {
                v.push(TxTrace::new(
                    $action,
                    info.to_string(),
                    unix_time_as_millis(),
                ))
            });
        }
    };
}

#[derive(Clone, Debug)]
pub struct TxTraceMap {
    inner: LruCache<H256, Vec<TxTrace>>,
}

impl TxTraceMap {
    pub fn new(capacity: usize) -> Self {
        TxTraceMap {
            inner: LruCache::new(capacity),
        }
    }

    pub fn add_pending<S: ToString>(&mut self, hash: &H256, info: S) {
        self.inner
            .entry(hash.clone())
            .or_insert_with(Vec::new)
            .push(TxTrace::new(
                Action::AddPending,
                info.to_string(),
                unix_time_as_millis(),
            ));
    }

    pub fn get(&self, hash: &H256) -> Option<&Vec<TxTrace>> {
        self.inner.get(hash)
    }

    define_method!(proposed, Action::Proposed);
    define_method!(staged, Action::Staged);
    define_method!(add_orphan, Action::AddOrphan);
    define_method!(expired, Action::Expired);
    define_method!(committed, Action::Committed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::transaction::TransactionBuilder;

    #[test]
    fn traces_fmt() {
        let mut map = TxTraceMap::new(100);
        let tx = TransactionBuilder::default().build();
        let tx_hash = tx.hash();

        let faketime_file = faketime::millis_tempfile(9102).expect("create faketime file");
        faketime::enable(&faketime_file);

        map.add_pending(&tx_hash, "pending");
        map.proposed(&tx_hash, "proposed");
        map.staged(&tx_hash, "staged");
        map.add_orphan(&tx_hash, "add_orphan");
        map.expired(&tx_hash, "expired");
        map.committed(&tx_hash, "committed");

        let traces = map.get(&tx_hash);

        assert_eq!(
            format!("{:?}", traces),
            concat!(
                "Some([",
                "{ action: AddPending, info: pending, time: 9102 }, ",
                "{ action: Proposed, info: proposed, time: 9102 }, ",
                "{ action: Staged, info: staged, time: 9102 }, ",
                "{ action: AddOrphan, info: add_orphan, time: 9102 }, ",
                "{ action: Expired, info: expired, time: 9102 }, ",
                "{ action: Committed, info: committed, time: 9102 }",
                "])"
            ),
        );
    }
}
