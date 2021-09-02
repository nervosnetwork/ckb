use ckb_logger::Level;

mod utils;

#[test]
fn has_panic_info() {
    let (config, _tmp_dir) = utils::config_in_tempdir(|_| {});
    let log_file = config.log_dir.join(config.file.as_path());
    let line_content = "test has panic info";
    let panic_message = "panic first";
    let panic_line_content_1 = format!(r"thread 'unnamed' panicked at '{}':.*", panic_message);
    let panic_line_content_2 = r"thread 'unnamed' panicked at 'panic second':.*";
    utils::do_tests(config, || {
        let _ = ::std::thread::spawn(move || {
            panic!("{}", panic_message);
        })
        .join();
        let _ = ::std::thread::spawn(move || {
            panic!("panic second");
        })
        .join();
        ckb_logger::error!("{}", line_content);
    });

    utils::test_if_log_file_exists(&log_file, true);

    assert!(utils::has_line_in_log_file(
        &log_file,
        Level::Error,
        &panic_line_content_1
    ));

    assert!(utils::has_line_in_log_file(
        &log_file,
        Level::Error,
        &panic_line_content_2
    ));

    assert!(utils::has_line_in_log_file(
        &log_file,
        Level::Error,
        &line_content
    ));
}
