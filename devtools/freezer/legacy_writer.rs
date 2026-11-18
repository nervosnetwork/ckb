// Writes the pre-Freezer hot schema with an independently linked old engine.
use rocksdb::{
    ops::{CompactRangeCF, GetPropertyCF},
    prelude::*,
    DBCompressionType, Options, WriteBatch, WriteOptions, DB,
};
use std::{
    error::Error,
    fs,
    io::{Cursor, Read},
    path::Path,
};

fn number(input: &mut Cursor<Vec<u8>>) -> std::io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 3, "usage: legacy-hot-db SEED NEW_DB sst|wal");
    assert!(!Path::new(&args[1]).exists());
    assert!(["sst", "wal"].contains(&args[2].as_str()));
    let mut input = Cursor::new(fs::read(&args[0])?);
    let mut rows = Vec::new();
    while input.position() < input.get_ref().len() as u64 {
        let col = number(&mut input)?.to_string();
        let mut key = vec![0; number(&mut input)? as usize];
        let mut value = vec![0; number(&mut input)? as usize];
        input.read_exact(&mut key)?;
        input.read_exact(&mut value)?;
        rows.push((col, key, value));
    }
    let mut options = Options::default();
    options.create_if_missing(true);
    options.set_compression_type(DBCompressionType::Snappy);
    options.set_disable_auto_compactions(true);
    let mut db = DB::open(&options, &args[1])?;
    for col in 0..19 {
        db.create_cf(&col.to_string(), &options)?;
    }
    let mut write_options = WriteOptions::default();
    write_options.set_sync(true);
    for parity in 0..2 {
        let mut batch = WriteBatch::default();
        for (index, (col, key, value)) in rows.iter().enumerate() {
            if index % 2 == parity {
                batch.put_cf(db.cf_handle(col).unwrap(), key, value)?;
            }
        }
        db.write_opt(&batch, &write_options)?;
        if parity == 0 || args[2] == "sst" {
            for col in 0..19 {
                let cf = db.cf_handle(&col.to_string()).unwrap();
                db.compact_range_cf(cf, None, None);
                assert_eq!(
                    db.property_int_value_cf(cf, "rocksdb.num-entries-active-mem-table")?,
                    Some(0)
                );
                assert_eq!(
                    db.property_int_value_cf(cf, "rocksdb.num-immutable-mem-table")?,
                    Some(0)
                );
            }
        }
    }
    println!(
        "legacy-binding={} rows={} mode={}",
        env!("CARGO_PKG_VERSION"),
        rows.len(),
        args[2]
    );
    // Prove the second batch is still in memory and relies on its synced WAL.
    if args[2] == "wal" {
        let mut entries = 0;
        for col in 0..19 {
            let cf = db.cf_handle(&col.to_string()).unwrap();
            entries += db
                .property_int_value_cf(cf, "rocksdb.num-entries-active-mem-table")?
                .unwrap();
        }
        assert_eq!(entries, (rows.len() / 2) as u64);
        println!("synced-wal-entries={entries}");
        std::process::exit(0);
    }
    Ok(())
}
