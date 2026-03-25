#[test]
fn logging_mode_off_disables_operational_tracing() {
    let result = smelly_connect_cli::logging::init_for_test("off", "info", None);
    assert!(result.is_ok());
}

#[test]
fn logging_mode_stdout_file_initializes_dual_sink() {
    let result =
        smelly_connect_cli::logging::init_for_test("stdout+file", "info", Some("test.log"));
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

#[test]
fn pool_logs_prewarm_summary_and_ready_events() {
    let events = smelly_connect_cli::logging::capture_pool_events_for_test();
    assert!(
        events
            .iter()
            .any(|line| line.contains("pool prewarm start"))
    );
    assert!(
        events
            .iter()
            .any(|line| line.contains("pool startup summary"))
    );
    assert!(events.iter().any(|line| line.contains("account ready")));
}

#[test]
fn http_request_logs_protocol_target_and_account() {
    let events = smelly_connect_cli::logging::capture_http_request_log_for_test();
    assert!(events.iter().any(|line| line.contains("protocol=http")));
    assert!(events.iter().any(|line| line.contains("account=acct-01")));
}

#[test]
fn http_connect_request_logs_protocol_connect_and_account() {
    let events = smelly_connect_cli::logging::capture_http_connect_log_for_test();
    assert!(events.iter().any(|line| line.contains("protocol=connect")));
    assert!(events.iter().any(|line| line.contains("account=acct-01")));
}

#[test]
fn socks5_request_logs_protocol_target_and_account() {
    let events = smelly_connect_cli::logging::capture_socks5_request_log_for_test();
    assert!(events.iter().any(|line| line.contains("protocol=socks5")));
    assert!(events.iter().any(|line| line.contains("account=acct-01")));
}

#[test]
fn no_ready_session_fast_fail_emits_warn_log() {
    let events = smelly_connect_cli::logging::capture_no_ready_session_warn_for_test();
    assert!(events.iter().any(|line| line.contains(" WARN ")));
    assert!(events.iter().any(|line| line.contains("no ready session")));
}

#[test]
fn file_mode_falls_back_to_stderr_when_file_open_fails() {
    let result = smelly_connect_cli::logging::init_for_test(
        "file",
        "info",
        Some("/definitely/not/writable/log.txt"),
    );
    assert!(result.is_ok());
}

#[test]
fn config_load_failure_emits_error_log() {
    let events = smelly_connect_cli::logging::capture_config_load_error_for_test(
        "/definitely/missing/config.toml",
    );
    assert!(events.iter().any(|line| line.contains(" ERROR ")));
}

#[test]
fn invalid_logging_config_emits_error_log() {
    let events = smelly_connect_cli::logging::capture_invalid_logging_config_error_for_test();
    assert!(events.iter().any(|line| line.contains(" ERROR ")));
    assert!(events.iter().any(|line| line.contains("logging")));
}
