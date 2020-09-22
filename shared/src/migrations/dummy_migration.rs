use std::{thread, time};

use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};

pub struct DummyMigration {
    tag: String,
    version: String,
    intervals: Vec<u64>, // milliseconds
}

impl DummyMigration {
    pub fn new(tag: &str, version: &str, intervals: &[u64]) -> Self {
        Self {
            tag: tag.to_owned(),
            version: version.to_owned(),
            intervals: intervals.to_owned(),
        }
    }
}

impl Migration for DummyMigration {
    fn migrate(&self, db: RocksDB, mut pb: Box<dyn FnMut(u64) -> ProgressBar>) -> Result<RocksDB> {
        let pb = pb((self.intervals.len() as u64) * 2 + 1);
        let spinner_style = ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
            .template("{prefix:.bold.dim} {spinner} {wide_msg}");
        pb.set_style(spinner_style);

        for (i, ms) in self.intervals.iter().enumerate() {
            pb.set_message(&format!(
                "migrating: {} step {}: sleep {} ms",
                self.tag, i, ms
            ));
            pb.inc(1);
            let interval = time::Duration::from_millis(*ms);
            thread::sleep(interval);
            pb.set_message(&format!("finish: {} step {}", self.tag, i));
            pb.inc(1);
        }

        pb.set_message("commit changes");
        pb.inc(1);
        pb.finish_with_message("waiting...");
        Ok(db)
    }

    fn version(&self) -> &str {
        &self.version
    }
}
