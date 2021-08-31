use ckb_logger::Level;

mod utils;

#[test]
fn env_filter() {
    let (config, _tmp_dir) = utils::config_in_tempdir(|config| {
        config.filter = None;
    });
    let log_file = config.log_dir.join(config.file.as_path());
    let line_content = "test env filter";
    let env_level = Level::Debug;
    utils::do_tests_with_env(env_level.as_str(), config, || {
        utils::output_log_for_all_log_levels(line_content);
    });

    utils::test_if_log_file_exists(&log_file, true);

    for level in utils::all_log_levels() {
        assert_eq!(
            *level <= env_level,
            utils::has_line_in_log_file(&log_file, *level, line_content)
        );
    }
}
