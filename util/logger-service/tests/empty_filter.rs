use ckb_logger::Level;

mod utils;

#[test]
fn empty_filter() {
    let (config, _tmp_dir) = utils::config_in_tempdir(|config| {
        config.filter = None;
    });
    let log_file = config.log_dir.join(config.file.as_path());
    let line_content = "test empty filter";
    utils::do_tests(config, || {
        utils::output_log_for_all_log_levels(line_content);
    });

    utils::test_if_log_file_exists(&log_file, true);

    for level in utils::all_log_levels() {
        assert_eq!(
            *level <= Level::Error,
            utils::has_line_in_log_file(&log_file, *level, line_content)
        );
    }
}
