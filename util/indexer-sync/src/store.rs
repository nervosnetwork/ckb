use crate::error::Error;
use ckb_db_schema::{
    COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_INDEX, COLUMN_META, Col,
    META_ARCHIVE_NEXT_RECORD, freezer,
};
use ckb_store::{ChainStore, Freezer, FreezerReader, StoreCache};
use rocksdb::{
    ColumnFamilyDescriptor, DBIterator, DBPinnableSlice, IteratorMode,
    SecondaryDB as SecondaryRocksDB, SecondaryOpenDescriptor, ops::OpenCF, prelude::*,
};
use std::{path::Path, sync::Arc};

pub(super) const INDEXER_COLUMNS: [Col; 7] = [
    COLUMN_INDEX,
    COLUMN_META,
    COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_BODY,
    COLUMN_BLOCK_UNCLE,
    COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_EXTENSION,
];

/// A native Secondary opened on the fixed columns or a validated Freezer generation.
#[derive(Clone)]
pub struct SecondaryDB {
    inner: Arc<SecondaryRocksDB>,
    archive: Option<ArchiveRoute>,
}

#[derive(Clone)]
struct ArchiveRoute {
    generation: u64,
    segments: freezer::Segments,
    reader: Arc<FreezerReader>,
}

impl SecondaryDB {
    /// Open a fixed-column Secondary.
    pub fn open_cf<P, I, N>(opts: &Options, path: P, cf_names: I, secondary_path: String) -> Self
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = N>,
        N: Into<String>,
    {
        Self::open(
            opts,
            path.as_ref(),
            cf_names.into_iter().map(Into::into),
            secondary_path,
        )
        .expect("Failed to open SecondaryDB")
    }

    fn open(
        opts: &Options,
        path: &Path,
        names: impl IntoIterator<Item = String>,
        secondary_path: String,
    ) -> Result<Self, Error> {
        let descriptors = names
            .into_iter()
            .map(|name| ColumnFamilyDescriptor::new(name, Options::default()));
        let inner = SecondaryRocksDB::open_cf_descriptors_with_descriptor(
            opts,
            path,
            descriptors,
            SecondaryOpenDescriptor::new(secondary_path),
        )?;
        Ok(Self {
            inner: Arc::new(inner),
            archive: None,
        })
    }

    pub(super) fn open_with_archive(
        primary: &Path,
        secondary: &Path,
        ancient: &Path,
    ) -> Result<Self, Error> {
        let mut opts = super::indexer_secondary_options();
        opts.set_max_open_files(-1);
        // Discovery may race creation/drop. The marker in the *opened* DB,
        // never the greatest name in this list, selects the reader's generation.
        let names = rocksdb::DB::list_cf(&opts, primary)?
            .into_iter()
            .filter(|name| {
                INDEXER_COLUMNS.contains(&freezer::logical_name(name).unwrap_or(name))
                    || name == COLUMN_BLOCK_ARCHIVE
                    || freezer::parse_segment(name).is_some()
            });
        let mut view = Self::open(
            &opts,
            primary,
            names,
            secondary.to_string_lossy().into_owned(),
        )?;
        view.archive = Some(ArchiveRoute {
            generation: view.generation()?,
            segments: view.epoch_layout()?,
            reader: Arc::new(FreezerReader::open(ancient)?),
        });
        view.validate_route(ancient)?;
        Ok(view)
    }

    fn epoch_layout(&self) -> Result<freezer::Segments, Error> {
        freezer::Segments::decode(
            self.inner.get_pinned(freezer::SEGMENTS_KEY)?.as_deref(),
            None,
        )
        .map_err(|e| Error::DB(e.into()))
    }

    fn segment_handle(&self, start: u64) -> Result<&rocksdb::ColumnFamily, ckb_error::Error> {
        if self
            .archive
            .as_ref()
            .is_none_or(|route| route.segments.windows().binary_search(&start).is_err())
        {
            return Err(ckb_error::InternalErrorKind::DataCorrupted
                .other("route refers to an unpublished epoch segment")
                .into());
        }
        self.inner
            .cf_handle(&freezer::segment_name(start))
            .ok_or_else(|| {
                ckb_error::InternalErrorKind::DataCorrupted
                    .other("missing epoch column")
                    .into()
            })
    }

    fn generation(&self) -> Result<u64, Error> {
        freezer::generation(self.inner.get_pinned(freezer::GENERATION_KEY)?.as_deref())
            .map_err(|_| Error::DB("invalid Freezer generation".into()))
    }

    fn handle(&self, col: Col) -> Result<&rocksdb::ColumnFamily, Error> {
        let physical;
        let name = match &self.archive {
            Some(route) if route.generation != 0 && freezer::PAYLOAD_COLUMNS.contains(&col) => {
                physical = freezer::physical_name(route.generation, col);
                physical.as_str()
            }
            _ => col,
        };
        self.inner
            .cf_handle(name)
            .ok_or_else(|| Error::DB(format!("column {name} not found")))
    }

    pub(super) fn validate_route(&mut self, ancient: &Path) -> Result<(), Error> {
        let generation = self.generation()?;
        let Some(route) = &self.archive else {
            return Err(Error::DB("missing Freezer route".into()));
        };
        if self.epoch_layout()? != route.segments {
            return Err(Error::DB("epoch segment set changed".into()));
        }
        for &start in route.segments.windows() {
            let cf = self
                .inner
                .cf_handle(&freezer::segment_name(start))
                .ok_or_else(|| Error::DB("missing epoch segment".into()))?;
            if self
                .inner
                .property_int_value_cf(cf, "rocksdb.is-column-family-dropped")?
                != Some(0)
            {
                return Err(Error::DB("inactive epoch segment".into()));
            }
        }
        if generation != route.generation {
            return Err(Error::DB("Freezer generation changed".into()));
        }
        for col in INDEXER_COLUMNS.into_iter().chain([COLUMN_BLOCK_ARCHIVE]) {
            // A same-name replacement is not the handle that this Secondary opened.
            if self
                .inner
                .property_int_value_cf(self.handle(col)?, "rocksdb.is-column-family-dropped")?
                != Some(0)
            {
                return Err(Error::DB(format!("inactive Secondary column {col}")));
            }
        }
        let next = u64::from_le_bytes(
            self.get_pinned(COLUMN_META, META_ARCHIVE_NEXT_RECORD)?
                .as_deref()
                .ok_or_else(|| Error::DB("missing archive cursor".into()))?
                .try_into()
                .map_err(|_| Error::DB("invalid archive cursor".into()))?,
        );
        if next == 0 {
            return Err(Error::DB("zero archive cursor".into()));
        }
        if next > route.reader.number() {
            let reader = FreezerReader::open(ancient)?;
            if next > reader.number() {
                return Err(Error::DB(
                    "archive does not cover the Secondary cursor".into(),
                ));
            }
            self.archive = Some(ArchiveRoute {
                generation,
                segments: route.segments.clone(),
                reader: Arc::new(reader),
            });
        }
        Ok(())
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the given column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice<'_>>, Error> {
        let cf = self.handle(col)?;
        self.inner.get_pinned_cf(cf, key).map_err(Into::into)
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
    fn iter(&self, col: Col, mode: IteratorMode) -> Result<DBIterator<'_>, Error> {
        let opts = ReadOptions::default();
        let cf = self.handle(col)?;
        self.inner
            .iterator_cf_opt(cf, mode, &opts)
            .map_err(Into::into)
    }
}

impl ChainStore for SecondaryDB {
    fn segment_value(
        &self,
        start: u64,
        key: &[u8],
    ) -> Result<Option<DBPinnableSlice<'_>>, ckb_error::Error> {
        self.inner
            .get_pinned_cf(self.segment_handle(start)?, key)
            .map_err(|e| ckb_error::InternalErrorKind::Database.other(e).into())
    }

    fn segment_rows(
        &self,
        start: u64,
        mode: IteratorMode,
    ) -> Result<DBIterator<'_>, ckb_error::Error> {
        self.inner
            .iterator_cf(self.segment_handle(start)?, mode)
            .map_err(|e| ckb_error::InternalErrorKind::Database.other(e).into())
    }
    fn cache(&self) -> Option<&StoreCache> {
        None
    }

    fn freezer(&self) -> Option<&Freezer> {
        None
    }

    fn archive_reader(&self) -> Option<&FreezerReader> {
        self.archive.as_ref().map(|route| route.reader.as_ref())
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice<'_>> {
        self.get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIterator<'_> {
        self.iter(col, mode).expect("db operation should be ok")
    }
}
