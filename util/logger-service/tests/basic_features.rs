mod utils;

#[test]
fn basic_features() {
    let (config, _tmp_dir) = utils::config_in_tempdir(|_| {});
    let log_file = config.log_dir.join(config.file.as_path());
    let line_content = "test basic features";
    utils::do_tests(config, || {
        utils::output_log_for_all_log_levels(line_content);
    });

    utils::test_if_log_file_exists(&log_file, true);

    for level in utils::all_log_levels() {
        assert!(utils::has_line_in_log_file(&log_file, *level, line_content));
    }
}
