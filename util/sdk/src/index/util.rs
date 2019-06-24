use std::fs;
use std::path::Path;

pub(crate) fn put_pair(
    store: rkv::SingleStore,
    writer: &mut rkv::Writer,
    (key, value): (Vec<u8>, Vec<u8>),
) {
    store.put(writer, key, &rkv::Value::Blob(&value)).unwrap();
}

pub(crate) fn value_to_bytes<'a>(value: &'a rkv::Value) -> &'a [u8] {
    match value {
        rkv::Value::Blob(inner) => inner,
        _ => panic!("Invalid value type: {:?}", value),
    }
}

pub(crate) fn dir_size<P: AsRef<Path>>(path: P) -> u64 {
    let mut total_size = 0;
    for dir_entry in fs::read_dir(path.as_ref()).unwrap() {
        let metadata = dir_entry.unwrap().metadata().unwrap();
        total_size += metadata.len();
    }
    total_size
}
