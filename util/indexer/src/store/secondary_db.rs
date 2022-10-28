use crate::error::Error;
use ckb_db_schema::Col;
use ckb_store::{ChainStore, Freezer, StoreCache};
use ckb_types::{core::BlockView, packed, prelude::*};
use rocksdb::{
    ops::OpenCF, prelude::*, ColumnFamilyDescriptor, DBIterator, DBPinnableSlice, IteratorMode,
    SecondaryDB as SecondaryRocksDB, SecondaryOpenDescriptor,
};
use std::path::Path;
use std::sync::Arc;

/// Open DB as secondary instance with specified column families
//
// When opening DB in secondary mode, you can specify only a subset of column
// families in the database that should be opened. However, you always need
// to specify default column family. The default column family name is
// 'default' and it's stored in ROCKSDB_NAMESPACE::kDefaultColumnFamilyName
//
// Column families created by the primary after the secondary instance starts
// are currently ignored by the secondary instance.  Column families opened
// by secondary and dropped by the primary will be dropped by secondary as
// well (on next invocation of TryCatchUpWithPrimary()). However the user
// of the secondary instance can still access the data of such dropped column
// family as long as they do not destroy the corresponding column family
// handle.
//
// The options argument specifies the options to open the secondary instance.
// Options.max_open_files should be set to -1.
// The name argument specifies the name of the primary db that you have used
// to open the primary instance.
// The secondary_path argument points to a directory where the secondary
// instance stores its info log.
// The column_families argument specifies a list of column families to open.
// If default column family is not specified or if any specified column
// families does not exist, the function returns non-OK status.

// Notice: rust-rocksdb `OpenRaw` handle 'default' column automatically
#[derive(Clone)]
pub(crate) struct SecondaryDB {
    inner: Arc<SecondaryRocksDB>,
}

impl SecondaryDB {
    /// Open a SecondaryDB
    pub fn open_cf<P, I, N>(path: P, cf_names: I, secondary_path: String) -> Self
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = N>,
        N: Into<String>,
    {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cf_descriptors: Vec<_> = cf_names
            .into_iter()
            .map(|name| ColumnFamilyDescriptor::new(name, Options::default()))
            .collect();

        let descriptor = SecondaryOpenDescriptor::new(secondary_path);
        let inner = SecondaryRocksDB::open_cf_descriptors_with_descriptor(
            &opts,
            path,
            cf_descriptors,
            descriptor,
        )
        .expect("Failed to open SecondaryDB");
        SecondaryDB {
            inner: Arc::new(inner),
        }
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the given column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>, Error> {
        let cf = self
            .inner
            .cf_handle(col)
            .ok_or_else(|| Error::DB(format!("column {} not found", col)))?;
        self.inner.get_pinned_cf(cf, &key).map_err(Into::into)
    }

    /// Make the secondary instance catch up with the primary by tailing and
    /// replaying the MANIFEST and WAL of the primary.
    // Column families created by the primary after the secondary instance starts
    // will be ignored unless the secondary instance closes and restarts with the
    // newly created column families.
    // Column families that exist before secondary instance starts and dropped by
    // the primary afterwards will be marked as dropped. However, as long as the
    // secondary instance does not delete the corresponding column family
    // handles, the data of the column family is still accessible to the
    // secondary.
    pub fn try_catch_up_with_primary(&self) -> Result<(), Error> {
        self.inner.try_catch_up_with_primary().map_err(Into::into)
    }

    /// This is used when you want to iterate over a specific ColumnFamily
    fn iter(&self, col: Col, mode: IteratorMode) -> Result<DBIterator, Error> {
        let opts = ReadOptions::default();
        let cf = self
            .inner
            .cf_handle(col)
            .ok_or_else(|| Error::DB(format!("column {} not found", col)))?;
        self.inner
            .iterator_cf_opt(cf, mode, &opts)
            .map_err(Into::into)
    }
}

impl<'a> ChainStore<'a> for SecondaryDB {
    type Vector = DBPinnableSlice<'a>;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        None
    }

    fn freezer(&'a self) -> Option<&'a Freezer> {
        None
    }

    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIterator {
        self.iter(col, mode).expect("db operation should be ok")
    }

    // Only block header and block body loaded
    fn get_block(&'a self, h: &packed::Byte32) -> Option<BlockView> {
        let header = self.get_block_header(h)?;
        let body = self.get_block_body(h);
        let uncles = packed::UncleBlockVecView::default();
        let proposals = packed::ProposalShortIdVec::default();
        Some(BlockView::new_unchecked(
            header,
            uncles.unpack(),
            body,
            proposals,
        ))
    }
}
