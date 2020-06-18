use crate::freezer_files::FreezerFilesBuilder;

fn make_bytes(size: usize, byte: u8) -> Vec<u8> {
    let mut ret = Vec::with_capacity(size);
    ret.resize_with(size, || byte);
    ret
}

#[test]
fn basic() {
    let tempdir = tempfile::Builder::new().tempdir().unwrap();

    let mut freezer = FreezerFilesBuilder::new(tempdir.path().to_path_buf())
        .max_file_size(50)
        .build()
        .unwrap();

    for i in 1..255 {
        let data = make_bytes(15, i);
        freezer.append(i.into(), &data).unwrap();
    }

    // for chunk in freezer.read_all_index().unwrap().chunks_exact(12) {
    //     println!("index {:?}", chunk);
    // }

    for i in 1..255 {
        let expect = make_bytes(15, i);
        let actual = freezer.retrieve(i.into()).unwrap();
        assert_eq!(expect, actual);
    }
}
