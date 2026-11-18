//! A completed Secondary view is leased for one indexed block. The node never
//! takes this lock; a slow indexer can hold its own files, not a Primary snapshot.
use super::{Error, SecondaryDB};
use ckb_store::ChainStore;
use ckb_types::{
    core::{BlockView, HeaderView},
    packed::Byte32,
    prelude::*,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

/// Node data used by the built-in indexers.
#[derive(Clone)]
pub struct IndexerSource(Arc<Source>);

enum Source {
    Fixed(SecondaryDB),
    Freezer {
        primary: PathBuf,
        secondary: PathBuf,
        ancient: PathBuf,
        view: RwLock<Option<SecondaryDB>>,
    },
}

impl From<SecondaryDB> for IndexerSource {
    fn from(view: SecondaryDB) -> Self {
        Self(Arc::new(Source::Fixed(view)))
    }
}

impl IndexerSource {
    /// Discover the published layout on first refresh. Both stores must refer
    /// to the same node; replacing a restored pair requires restarting readers.
    pub fn with_freezer(primary: &Path, secondary: &Path, ancient: &Path) -> Self {
        Self(Arc::new(Source::Freezer {
            primary: primary.into(),
            secondary: secondary.into(),
            ancient: ancient.into(),
            view: RwLock::new(None),
        }))
    }

    pub(crate) fn refresh(&self) -> Result<(), Error> {
        match &*self.0 {
            Source::Fixed(view) => view.try_catch_up_with_primary(),
            Source::Freezer {
                primary,
                secondary,
                ancient,
                view,
            } => {
                let mut slot = view.write().expect("secondary lock");
                if let Some(view) = slot.as_mut()
                    && view
                        .try_catch_up_with_primary()
                        .and_then(|_| view.validate_route(ancient))
                        .is_ok()
                {
                    return Ok(());
                }
                // Catch-up mutates in place, even on failure. Discard the old
                // instance before opening the same secondary log directory.
                *slot = None;
                *slot = Some(SecondaryDB::open_with_archive(primary, secondary, ancient)?);
                Ok(())
            }
        }
    }

    fn read<T>(&self, read: impl FnOnce(&SecondaryDB) -> Result<T, Error>) -> Result<T, Error> {
        match &*self.0 {
            Source::Fixed(view) => read(view),
            Source::Freezer { view, .. } => {
                let lease = view.read().expect("secondary lock");
                read(
                    lease
                        .as_ref()
                        .ok_or_else(|| Error::DB("Secondary awaiting refresh".into()))?,
                )
            }
        }
    }

    pub(crate) fn block(&self, number: u64) -> Result<Option<BlockView>, Error> {
        self.read(|view| {
            let Some(hash) = view.get_block_hash(number) else {
                return Ok(None);
            };
            if view.archive_reader().is_none() {
                return Ok(view.get_block(&hash));
            }
            let block = view
                .try_get_block(&hash)
                .map_err(|e| Error::DB(e.to_string()))?
                .ok_or_else(|| Error::DB(format!("missing canonical header at {number}")))?;
            // Native WAL replay does not promise an atomic view across CFs.
            // Recompute from payload bytes before passing data to either index.
            let block = block.data().into_view_without_reset_header();
            if block.hash() != hash
                || block.number() != number
                || block.calc_transactions_root() != block.transactions_root()
                || block.calc_proposals_hash() != block.proposals_hash()
                || block.calc_extra_hash().extra_hash() != block.extra_hash()
            {
                return Err(Error::DB(format!(
                    "inconsistent Secondary block at {number} ({hash})"
                )));
            }
            Ok(Some(block))
        })
    }

    pub(crate) fn canonical_hash(&self, number: u64) -> Result<Option<Byte32>, Error> {
        self.read(|view| Ok(view.get_block_hash(number)))
    }

    pub(crate) fn header(&self, hash: &Byte32) -> Result<Option<HeaderView>, Error> {
        self.read(|view| Ok(view.get_block_header(hash)))
    }
    pub(crate) fn tip(&self) -> Result<Option<HeaderView>, Error> {
        self.read(|view| Ok(view.get_tip_header()))
    }
}
