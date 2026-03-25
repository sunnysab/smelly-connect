# smelly-connect-cli Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add configurable CLI-side text logging to `smelly-connect-cli`, with `stdout`, `file`, `stdout+file`, and `off` modes plus operational `info`/`warn`/`error` logging around startup, requests, and pool lifecycle.

**Architecture:** Keep logging entirely inside `smelly-connect-cli`: parse `[logging]` from `config.toml`, initialize a tracing subscriber in `main.rs`, and emit operational events from the pool and proxy layers. Use simple text logging with append-mode file output and a lightweight fallback to `stderr`/`stdout` when file sink setup fails.

**Tech Stack:** Rust stable, `smelly-connect-cli`, `tracing`, `tracing-subscriber`, Tokio, Serde/TOML, `cargo test`, `cargo clippy`.

---

## Planned File Structure

### Create

- `smelly-connect-cli/src/logging.rs`
- `smelly-connect-cli/tests/logging_config.rs`
- `smelly-connect-cli/tests/logging_runtime.rs`
- `smelly-connect-cli/tests/fixtures/config.logging.stdout.toml`
- `smelly-connect-cli/tests/fixtures/config.logging.file.toml`
- `smelly-connect-cli/tests/fixtures/config.logging.off.toml`

### Modify

- `smelly-connect-cli/Cargo.toml`
- `smelly-connect-cli/src/config.rs`
- `smelly-connect-cli/src/lib.rs`
- `smelly-connect-cli/src/main.rs`
- `smelly-connect-cli/src/pool.rs`
- `smelly-connect-cli/src/commands/proxy.rs`
- `smelly-connect-cli/src/proxy/http.rs`
- `smelly-connect-cli/src/proxy/socks5.rs`
- `config.toml.example`
- `README.md`
- `docs/superpowers/specs/2026-03-26-smelly-connect-cli-logging-design.md`

## Task 1: Add Logging Config Types And Defaults

**Files:**
- Modify: `smelly-connect-cli/Cargo.toml`
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect-cli/src/lib.rs`
- Test: `smelly-connect-cli/tests/logging_config.rs`
- Test Data: `smelly-connect-cli/tests/fixtures/config.logging.stdout.toml`
- Test Data: `smelly-connect-cli/tests/fixtures/config.logging.file.toml`
- Test Data: `smelly-connect-cli/tests/fixtures/config.logging.off.toml`

- [ ] **Step 1: Write failing tests for logging config parsing**

```rust
#[test]
fn logging_defaults_to_stdout_info_and_default_file() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"
        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"
        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"
        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        "#,
    ).unwrap();
    assert_eq!(cfg.logging.mode.as_str(), "stdout");
    assert_eq!(cfg.logging.level.as_str(), "info");
    assert_eq!(cfg.logging.file, "smelly-connect.log");
}

#[test]
fn terminal_logging_mode_means_stderr_not_stdout() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"
        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"
        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"
        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        [logging]
        mode = "stdout"
        "#,
    ).unwrap();
    assert_eq!(cfg.logging.mode.as_str(), "stdout");
}

#[test]
fn logging_level_is_parsed_and_available_for_filtering() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"
        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"
        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"
        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        [logging]
        level = "error"
        "#,
    ).unwrap();
    assert_eq!(cfg.logging.level.as_str(), "error");
}

#[test]
fn invalid_logging_mode_is_rejected() {
    let cfg = r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"
        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"
        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"
        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        [logging]
        mode = "bogus"
    "#;
    assert!(toml::from_str::<smelly_connect_cli::config::AppConfig>(cfg).is_err());
}

#[test]
fn invalid_logging_level_is_rejected() {
    let cfg = r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"
        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"
        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"
        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        [logging]
        level = "bogus"
    "#;
    assert!(toml::from_str::<smelly_connect_cli::config::AppConfig>(cfg).is_err());
}
```

- [ ] **Step 2: Run the logging config tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test logging_config`
Expected: FAIL with missing logging config fields or defaulting behavior

- [ ] **Step 3: Add logging config models and defaults**

```rust
pub struct LoggingConfig {
    pub mode: LoggingMode,
    pub level: LoggingLevel,
    pub file: String,
}
```

- [ ] **Step 4: Run the logging config tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test logging_config`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/Cargo.toml smelly-connect-cli/src/config.rs smelly-connect-cli/src/lib.rs smelly-connect-cli/tests/logging_config.rs smelly-connect-cli/tests/fixtures/config.logging.stdout.toml smelly-connect-cli/tests/fixtures/config.logging.file.toml smelly-connect-cli/tests/fixtures/config.logging.off.toml
git commit -m "feat: add cli logging config"
```

## Task 2: Initialize Tracing In The CLI Entry Point

**Files:**
- Create: `smelly-connect-cli/src/logging.rs`
- Modify: `smelly-connect-cli/src/main.rs`
- Test: `smelly-connect-cli/tests/logging_runtime.rs`

- [ ] **Step 1: Write failing tests for logging mode initialization**

```rust
#[test]
fn logging_mode_off_disables_operational_tracing() {
    let result = smelly_connect_cli::logging::init_for_test("off", "info", None);
    assert!(result.is_ok());
}

#[test]
fn logging_mode_stdout_file_initializes_dual_sink() {
    let result = smelly_connect_cli::logging::init_for_test("stdout+file", "info", Some("test.log"));
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
```

- [ ] **Step 2: Run the logging runtime tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test logging_runtime`
Expected: FAIL with missing logging initializer

- [ ] **Step 3: Implement the CLI-side logging initializer**

```rust
pub fn init_logging(cfg: &LoggingConfig) -> Result<LoggingGuard, String> { /* use OpenOptions::new().create(true).append(true), tracing_subscriber::fmt, and writer composition for stdout/file/stdout+file */ }
```

This task must explicitly choose:

- append-mode file opening via `OpenOptions::new().create(true).append(true)`
- terminal logs on `stderr`
- dual-sink composition for `stdout+file`
- fallback from file failure to terminal logging
- scoped test capture using a local dispatcher instead of `set_global_default`
- a concrete timer and formatter settings so emitted logs contain timestamp + severity + target/module

- [ ] **Step 4: Run the logging runtime tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test logging_runtime`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/logging.rs smelly-connect-cli/src/main.rs smelly-connect-cli/tests/logging_runtime.rs
git commit -m "feat: initialize cli tracing"
```

## Task 3: Add Startup And Pool Lifecycle Logging

**Files:**
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/src/commands/proxy.rs`
- Test: `smelly-connect-cli/tests/logging_runtime.rs`

- [ ] **Step 1: Write failing tests for startup and pool events**

```rust
#[tokio::test]
async fn pool_logs_prewarm_summary_and_failures() {
    let events = smelly_connect_cli::logging::capture_pool_events_for_test().await;
    assert!(events.iter().any(|line| line.contains("prewarm")));
    assert!(events.iter().any(|line| line.contains("ready")));
}
```

- [ ] **Step 2: Run the logging runtime tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test logging_runtime`
Expected: FAIL with missing pool/startup log events

- [ ] **Step 3: Emit `info`/`warn` logs for startup and pool lifecycle**

```rust
tracing::info!(configured = accounts, ready = ready_count, "pool startup summary");
tracing::info!(account = account_name, "account ready");
tracing::warn!(account = account_name, error = %err, "account prewarm failed");
tracing::warn!(account = account_name, "retrying account after fixed-delay backoff");
tracing::error!(account = account_name, error = %err, "fatal account setup failure");
```

- [ ] **Step 4: Run the logging runtime tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test logging_runtime`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/src/commands/proxy.rs smelly-connect-cli/tests/logging_runtime.rs
git commit -m "feat: add startup and pool lifecycle logging"
```

## Task 4: Add Request-Level Logging For HTTP And SOCKS5

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`
- Test: `smelly-connect-cli/tests/socks5_proxy.rs`
- Test: `smelly-connect-cli/tests/logging_runtime.rs`

- [ ] **Step 1: Write failing tests for per-request logging**

```rust
#[tokio::test]
async fn http_request_logs_protocol_target_and_account() {
    let events = smelly_connect_cli::logging::capture_http_request_log_for_test().await;
    assert!(events.iter().any(|line| line.contains("protocol=http")));
    assert!(events.iter().any(|line| line.contains("account=acct-01")));
}

#[tokio::test]
async fn http_connect_request_logs_protocol_connect_and_account() {
    let events = smelly_connect_cli::logging::capture_http_connect_log_for_test().await;
    assert!(events.iter().any(|line| line.contains("protocol=connect")));
    assert!(events.iter().any(|line| line.contains("account=acct-01")));
}

#[tokio::test]
async fn socks5_request_logs_protocol_target_and_account() {
    let events = smelly_connect_cli::logging::capture_socks5_request_log_for_test().await;
    assert!(events.iter().any(|line| line.contains("protocol=socks5")));
    assert!(events.iter().any(|line| line.contains("account=acct-01")));
}

#[tokio::test]
async fn no_ready_session_fast_fail_emits_warn_log() {
    let events = smelly_connect_cli::logging::capture_no_ready_session_warn_for_test().await;
    assert!(events.iter().any(|line| line.contains(" WARN ")));
    assert!(events.iter().any(|line| line.contains("no ready session")));
}
```

- [ ] **Step 2: Run the affected tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test http_proxy --test socks5_proxy --test logging_runtime`
Expected: FAIL with missing request log events

- [ ] **Step 3: Emit request-level `info` and no-ready-session `warn` logs**

```rust
tracing::info!(protocol = "http", target = %target, account = %account_name, "request accepted");
tracing::info!(protocol = "connect", target = %target, account = %account_name, "connect tunnel accepted");
tracing::warn!(protocol = "socks5", "no ready session");
tracing::warn!(protocol = "http", "no ready session");
tracing::error!(protocol = "http", error = %err, "listener failed");
```

- [ ] **Step 4: Run the affected tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test http_proxy --test socks5_proxy --test logging_runtime`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect-cli/tests/socks5_proxy.rs smelly-connect-cli/tests/logging_runtime.rs
git commit -m "feat: add proxy request logging"
```

## Task 5: Add File Sink Fallback And Password-Safe Logging

**Files:**
- Modify: `smelly-connect-cli/src/logging.rs`
- Modify: `smelly-connect-cli/src/config.rs`
- Test: `smelly-connect-cli/tests/logging_runtime.rs`

- [ ] **Step 1: Write failing tests for file fallback and password redaction**

```rust
#[test]
fn file_mode_falls_back_to_stderr_or_stdout_when_file_open_fails() {
    let result = smelly_connect_cli::logging::init_for_test("file", "info", Some("/definitely/not/writable/log.txt"));
    assert!(result.is_ok());
}

#[test]
fn logged_config_does_not_expose_plaintext_passwords() {
    let rendered = smelly_connect_cli::config::redacted_summary_for_test();
    assert!(!rendered.contains("pass1"));
}

#[test]
fn config_load_failure_emits_error_log() {
    let events = smelly_connect_cli::logging::capture_config_load_error_for_test("/definitely/missing/config.toml");
    assert!(events.iter().any(|line| line.contains(" ERROR ")));
}

#[test]
fn invalid_logging_config_emits_error_log() {
    let events = smelly_connect_cli::logging::capture_invalid_logging_config_error_for_test();
    assert!(events.iter().any(|line| line.contains(" ERROR ")));
    assert!(events.iter().any(|line| line.contains("logging")));
}
```

- [ ] **Step 2: Run the logging runtime tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test logging_runtime`
Expected: FAIL with missing fallback or redaction behavior

- [ ] **Step 3: Implement file fallback and redacted summaries**

```rust
// fallback: file sink setup failure -> warn + continue with terminal stderr logging
```

- [ ] **Step 4: Run the logging runtime tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test logging_runtime`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/logging.rs smelly-connect-cli/src/config.rs smelly-connect-cli/tests/logging_runtime.rs
git commit -m "feat: add logging fallback and redaction"
```

## Task 6: Update Config Example, README, And Full Verification

**Files:**
- Modify: `config.toml.example`
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-03-26-smelly-connect-cli-logging-design.md`

- [ ] **Step 1: Write a failing doc/config smoke test**

```rust
#[test]
fn config_example_contains_logging_section() {
    let body = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../config.toml.example"),
    )
    .unwrap();
    assert!(body.contains("[logging]"));
}
```

- [ ] **Step 2: Run the smoke test to verify it fails**

Run: `cargo test -p smelly-connect-cli --test logging_config config_example_contains_logging_section -- --exact`
Expected: FAIL until the example config is updated

- [ ] **Step 3: Update docs and example config**

```toml
[logging]
mode = "stdout"
level = "info"
file = "smelly-connect.log"
```

- [ ] **Step 4: Run full verification**

Run: `cargo fmt --all --check`
Expected: PASS

Run: `cargo test --workspace`
Expected: PASS

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add config.toml.example README.md docs/superpowers/specs/2026-03-26-smelly-connect-cli-logging-design.md
git commit -m "feat: document cli logging"
```

## Notes For The Implementer

- Keep tracing initialization inside `smelly-connect-cli`; do not push logging sink selection into `smelly-connect`.
- Terminal logs should go to `stderr`; command result output should remain on `stdout`.
- `mode = "off"` disables operational tracing only; fatal command failures may still print direct `stderr` messages.
- File logging is append-only and must not introduce rotation in this plan.
- If file sink setup fails, prefer fallback behavior over aborting startup.
