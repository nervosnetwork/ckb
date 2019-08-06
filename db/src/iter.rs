use crate::db::cf_handle;
use crate::{from_db_error, Col, Result, RocksDB};
use rocksdb::{ops::IterateCF, Direction as RdbDirection, IteratorMode};

pub type DBIteratorItem = (Box<[u8]>, Box<[u8]>);

pub enum Direction {
    Forward,
    Reverse,
}

pub trait DBIterator {
    fn iter<'a>(
        &'a self,
        col: Col,
        from_key: &'a [u8],
        direction: Direction,
    ) -> Result<Box<Iterator<Item = DBIteratorItem> + 'a>>;
}

impl DBIterator for RocksDB {
    fn iter<'a>(
        &'a self,
        col: Col,
        from_key: &'a [u8],
        direction: Direction,
    ) -> Result<Box<Iterator<Item = DBIteratorItem> + 'a>> {
        let cf = cf_handle(&self.inner, col)?;
        let iter_direction = match direction {
            Direction::Forward => RdbDirection::Forward,
            Direction::Reverse => RdbDirection::Reverse,
        };
        let mode = IteratorMode::From(from_key, iter_direction);
        self.inner
            .iterator_cf(cf, mode)
            .map(|iter| Box::new(iter) as Box<_>)
            .map_err(from_db_error)
    }
}
