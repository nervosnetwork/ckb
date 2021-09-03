use ckb_logger::Level;
use ckb_logger_service::Logger;

mod utils;

#[test]
fn extra_loggers() {
    let logger_name_turn_off = "turn_off";
    let logger_name_no_log = "no_log";
    let logger_name_warn = "warn";
    let logger_name_updated = "updated";
    let logger_name_removed = "removed";
    let logger_name_inserted = "inserted";
    let (config, _tmp_dir) = utils::config_in_tempdir(|config| {
        utils::update_extra_logger(config, logger_name_turn_off, "trace");
        utils::update_extra_logger(config, logger_name_no_log, "off");
        utils::update_extra_logger(config, logger_name_warn, "warn");
        utils::update_extra_logger(config, logger_name_updated, "trace");
        utils::update_extra_logger(config, logger_name_removed, "trace");
    });
    let log_dir = config.log_dir.clone();
    let line_content = "test extra loggers";
    let new_level = Level::Info;
    utils::do_tests(config, || {
        Logger::update_extra_logger(logger_name_turn_off.to_owned(), "off".to_owned()).unwrap();
        Logger::update_extra_logger(
            logger_name_updated.to_owned(),
            new_level.as_str().to_owned(),
        )
        .unwrap();
        Logger::remove_extra_logger(logger_name_removed.to_owned()).unwrap();
        Logger::update_extra_logger(
            logger_name_inserted.to_owned(),
            new_level.as_str().to_owned(),
        )
        .unwrap();
        utils::apply_new_config();
        utils::output_log_for_all_log_levels(line_content);
    });

    {
        let log_file = utils::extra_logger_file(&log_dir, logger_name_turn_off);
        utils::test_if_log_file_exists(&log_file, true);
        for level in utils::all_log_levels() {
            assert!(!utils::has_line_in_log_file(
                &log_file,
                *level,
                line_content
            ),);
        }
    }

    {
        let log_file = utils::extra_logger_file(&log_dir, logger_name_no_log);
        utils::test_if_log_file_exists(&log_file, true);
        for level in utils::all_log_levels() {
            assert!(!utils::has_line_in_log_file(
                &log_file,
                *level,
                line_content
            ),);
        }
    }

    {
        let log_file = utils::extra_logger_file(&log_dir, logger_name_warn);
        utils::test_if_log_file_exists(&log_file, true);
        for level in utils::all_log_levels() {
            assert_eq!(
                *level <= Level::Warn,
                utils::has_line_in_log_file(&log_file, *level, line_content),
            );
        }
    }

    {
        let log_file = utils::extra_logger_file(&log_dir, logger_name_updated);
        utils::test_if_log_file_exists(&log_file, true);
        for level in utils::all_log_levels() {
            assert_eq!(
                *level <= new_level,
                utils::has_line_in_log_file(&log_file, *level, line_content),
            );
        }
    }

    {
        let log_file = utils::extra_logger_file(&log_dir, logger_name_removed);
        utils::test_if_log_file_exists(&log_file, true);
        for level in utils::all_log_levels() {
            assert!(!utils::has_line_in_log_file(
                &log_file,
                *level,
                line_content
            ),);
        }
    }

    {
        let log_file = utils::extra_logger_file(&log_dir, logger_name_inserted);
        utils::test_if_log_file_exists(&log_file, true);
        for level in utils::all_log_levels() {
            assert_eq!(
                *level <= new_level,
                utils::has_line_in_log_file(&log_file, *level, line_content),
            );
        }
    }
}
