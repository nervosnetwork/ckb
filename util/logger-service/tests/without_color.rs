use ckb_logger::Level;

mod utils;

#[test]
fn without_color() {
    let (config, _tmp_dir) = utils::config_in_tempdir(|config| {
        config.color = false;
    });
    let log_file = config.log_dir.join(config.file.as_path());
    let line_content = "test without color";
    utils::do_tests(config, || {
        ckb_logger::error!("{}", line_content);
    });

    utils::test_if_log_file_exists(&log_file, true);

    assert!(utils::has_line_in_log_file(
        &log_file,
        Level::Error,
        line_content
    ));
}
