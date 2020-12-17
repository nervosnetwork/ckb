use ckb_freezer::FreezerFilesBuilder;
use fail::FailScenario;
use std::thread;

fn make_bytes(size: usize, byte: u8) -> Vec<u8> {
    let mut ret = Vec::with_capacity(size);
    ret.resize_with(size, || byte);
    ret
}

macro_rules! fail {
    ($tests_name:ident, $failpoint:expr) => {
        #[test]
        fn $tests_name() {
            let tempdir = tempfile::Builder::new().tempdir().unwrap();
            let fpath = tempdir.path().to_path_buf();

            let tb = thread::Builder::new().name($failpoint.into());

            let handler = tb
                .spawn(move || {
                    let scenario = FailScenario::setup();

                    let mut freezer = FreezerFilesBuilder::new(fpath)
                        .max_file_size(50)
                        .build()
                        .unwrap();

                    freezer.preopen().unwrap();

                    for i in 1..20 {
                        let data = make_bytes(15, i);
                        freezer.append(i.into(), &data).unwrap();
                    }

                    fail::cfg($failpoint, "panic").unwrap();
                    let data = make_bytes(15, 10);
                    freezer.append(10, &data).unwrap();

                    scenario.teardown();
                })
                .unwrap();

            assert!(handler.join().is_err()); // panic()

            let mut freezer = FreezerFilesBuilder::new(tempdir.path().to_path_buf())
                .max_file_size(50)
                .build()
                .unwrap();
            freezer.preopen().unwrap();

            for i in 20..30 {
                let data = make_bytes(15, i);
                freezer.append(i.into(), &data).unwrap();
            }

            for i in 1..30 {
                let expect = make_bytes(15, i);
                let actual = freezer.retrieve(i.into()).unwrap();
                assert_eq!(Some(expect), actual);
            }
        }
    };
}

fail!(write_head, "write-head");
fail!(write_index, "write-index");
fail!(index_entry_encode, "IndexEntry encode");
fail!(append_unexpected_number, "append-unexpected-number");
fail!(open_read_only, "open_read_only");
fail!(open_truncated, "open_truncated");
