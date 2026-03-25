#[test]
fn logging_mode_off_disables_operational_tracing() {
    let result = smelly_connect_cli::logging::init_for_test("off", "info", None);
    assert!(result.is_ok());
}

#[test]
fn logging_mode_stdout_file_initializes_dual_sink() {
    let result = smelly_connect_cli::logging::init_for_test(
        "stdout+file",
        "info",
        Some("test.log"),
    );
    assert!(result.is_ok());
}

#[test]
fn logging_level_filter_suppresses_info_when_level_is_error() {
    let events = smelly_connect_cli::logging::capture_level_filter_for_test("error");
    assert!(!events.iter().any(|line| line.contains(" INFO ")));
}

#[test]
fn emitted_log_line_contains_timestamp_and_target() {
    let line = smelly_connect_cli::logging::capture_one_info_line_for_test();
    assert!(line.chars().take(4).all(|ch| ch.is_ascii_digit()));
    assert!(line.contains(" INFO "));
    assert!(line.contains("smelly_connect_cli"));
}
