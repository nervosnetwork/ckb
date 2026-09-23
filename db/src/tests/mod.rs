mod collect;
mod compression;
mod db;
mod db_with_ttl;
mod read_only_db;

fn assert_wal_bounded(path: &std::path::Path, limit: u64, mut pressure: impl FnMut()) {
    use std::{
        fs, thread,
        time::{Duration, Instant},
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let bytes: u64 = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
            .map(|path| match fs::metadata(path) {
                Ok(metadata) => metadata.len(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
                Err(error) => panic!("WAL metadata: {error}"),
            })
            .sum();
        if bytes < limit {
            return;
        }
        assert!(Instant::now() < deadline, "WAL stalled at {bytes} bytes");
        // A completed background flush needs another write to check WAL pressure.
        pressure();
        thread::sleep(Duration::from_millis(10));
    }
}
