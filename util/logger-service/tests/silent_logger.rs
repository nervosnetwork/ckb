mod utils;

#[test]
fn silent_logger() {
    let line_content = "test silent logger";
    utils::do_tests_with_silent_logger(|| {
        utils::output_log_for_all_log_levels(line_content);
    });
}
