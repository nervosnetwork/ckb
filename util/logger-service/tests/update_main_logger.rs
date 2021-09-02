use ckb_logger::Level;
use ckb_logger_service::Logger;

mod utils;

#[test]
fn update_main_logger() {
    let trace_filter = Level::Trace.as_str();
    let (config, _tmp_dir) = utils::config_in_tempdir(|config| {
        config.filter = Some(trace_filter.to_owned());
        config.log_to_file = true;
        config.log_to_stdout = false;
    });
    let log_file = config.log_dir.join(config.file.as_path());
    let line_content_1 = "test update main logger first";
    let line_content_2 = "test update main logger second";
    let line_content_3 = "test update main logger third";
    let line_content_4 = "test update main logger fourth";
    let new_level = Level::Info;
    utils::do_tests(config, || {
        utils::output_log_for_all_log_levels(line_content_1);

        Logger::update_main_logger(
            Some(new_level.as_str().to_owned()),
            Some(true),
            None,
            Some(false),
        )
        .unwrap();
        utils::apply_new_config();
        utils::output_log_for_all_log_levels(line_content_2);

        Logger::update_main_logger(Some(trace_filter.to_owned()), None, Some(false), None).unwrap();
        utils::apply_new_config();
        utils::output_log_for_all_log_levels(line_content_3);

        Logger::update_main_logger(None, None, Some(true), None).unwrap();
        utils::apply_new_config();
        utils::output_log_for_all_log_levels(line_content_4);
    });

    for level in utils::all_log_levels() {
        assert!(utils::has_line_in_log_file(
            &log_file,
            *level,
            line_content_1
        ),);
    }

    for level in utils::all_log_levels() {
        assert_eq!(
            *level <= new_level,
            utils::has_line_in_log_file(&log_file, *level, line_content_2),
        );
    }

    for level in utils::all_log_levels() {
        assert!(!utils::has_line_in_log_file(
            &log_file,
            *level,
            line_content_3
        ),);
    }

    for level in utils::all_log_levels() {
        assert!(utils::has_line_in_log_file(
            &log_file,
            *level,
            line_content_4
        ),);
    }
}
