# smelly-connect-cli Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a separate `smelly-connect-cli` workspace crate that consumes `smelly-connect` as a library and provides multi-command connection, testing, inspection, and proxy-serving workflows with an account pool.

**Architecture:** Keep `smelly-connect` as a pure library crate and create a new `smelly-connect-cli` application crate for config loading, command parsing, pool orchestration, and foreground HTTP/SOCKS5 proxy serving. Implement the CLI in thin layers: parse config and arguments, build the pool, select ready sessions in round-robin order, and drive proxy or test commands entirely through stable library APIs.

**Tech Stack:** Rust stable, Cargo workspace, Tokio, `smelly-connect`, a CLI parser crate such as `clap`, TOML + Serde config parsing, logging/tracing, `cargo test`, `cargo clippy`.

---

## Planned File Structure

### Create

- `smelly-connect-cli/Cargo.toml`
- `smelly-connect-cli/src/lib.rs`
- `smelly-connect-cli/src/main.rs`
- `smelly-connect-cli/src/cli.rs`
- `smelly-connect-cli/src/config.rs`
- `smelly-connect-cli/src/pool.rs`
- `smelly-connect-cli/src/commands/mod.rs`
- `smelly-connect-cli/src/commands/proxy.rs`
- `smelly-connect-cli/src/commands/test.rs`
- `smelly-connect-cli/src/commands/inspect.rs`
- `smelly-connect-cli/src/proxy/mod.rs`
- `smelly-connect-cli/src/proxy/http.rs`
- `smelly-connect-cli/src/proxy/socks5.rs`
- `smelly-connect-cli/tests/cli_config.rs`
- `smelly-connect-cli/tests/pool.rs`
- `smelly-connect-cli/tests/http_proxy.rs`
- `smelly-connect-cli/tests/socks5_proxy.rs`
- `smelly-connect-cli/tests/inspect.rs`
- `smelly-connect-cli/tests/test_commands.rs`
- `smelly-connect-cli/tests/fixtures/config.sample.toml`

### Modify

- `Cargo.toml`
- `README.md`
- `smelly-connect/README.md`
- `docs/superpowers/specs/2026-03-26-smelly-connect-cli-design.md`

## Task 1: Add The New Workspace Member And CLI Crate Skeleton

**Files:**
- Modify: `Cargo.toml`
- Create: `smelly-connect-cli/Cargo.toml`
- Create: `smelly-connect-cli/src/lib.rs`
- Create: `smelly-connect-cli/src/main.rs`
- Create: `smelly-connect-cli/src/cli.rs`
- Create: `smelly-connect-cli/src/commands/mod.rs`

- [ ] **Step 1: Verify the workspace does not yet contain `smelly-connect-cli`**

Run: `cargo metadata --no-deps --format-version 1 | rg 'smelly-connect-cli'`
Expected: no matches

- [ ] **Step 2: Confirm package-scoped check currently fails**

Run: `cargo check -p smelly-connect-cli`
Expected: FAIL because the package does not exist yet

- [ ] **Step 3: Add the workspace member and minimal CLI skeleton**

```toml
[workspace]
members = ["smelly-connect", "smelly-connect-cli", "smelly-tls"]
```

```rust
// smelly-connect-cli/src/main.rs
fn main() {
    eprintln!("smelly-connect-cli is not wired yet");
}

// smelly-connect-cli/src/lib.rs
pub mod cli;
pub mod commands;
```

- [ ] **Step 4: Run the smoke test to verify the new crate compiles**

Run: `cargo check -p smelly-connect-cli`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml smelly-connect-cli/Cargo.toml smelly-connect-cli/src/lib.rs smelly-connect-cli/src/main.rs smelly-connect-cli/src/cli.rs smelly-connect-cli/src/commands/mod.rs
git commit -m "feat: add smelly-connect-cli workspace crate"
```

## Task 2: Implement CLI Argument Parsing And Config File Loading

**Files:**
- Modify: `smelly-connect-cli/Cargo.toml`
- Modify: `smelly-connect-cli/src/lib.rs`
- Modify: `smelly-connect-cli/src/main.rs`
- Modify: `smelly-connect-cli/src/cli.rs`
- Create: `smelly-connect-cli/src/config.rs`
- Test: `smelly-connect-cli/tests/cli_config.rs`
- Test Data: `smelly-connect-cli/tests/fixtures/config.sample.toml`

- [ ] **Step 1: Write failing tests for argument parsing and `config.toml` loading**

```rust
#[test]
fn defaults_to_config_toml_in_cwd() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(["smelly-connect-cli", "proxy"]);
    assert_eq!(cli.config_path().to_string_lossy(), "config.toml");
}

#[test]
fn parses_sample_config() {
    let cfg: smelly_connect_cli::config::AppConfig =
        toml::from_str(include_str!("fixtures/config.sample.toml")).unwrap();
    assert_eq!(cfg.accounts.len(), 2);
    assert_eq!(cfg.pool.prewarm, 2);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test cli_config`
Expected: FAIL with missing CLI/config types or parser setup

- [ ] **Step 3: Implement the CLI model and TOML config structs**

```rust
pub struct AppConfig {
    pub vpn: VpnConfig,
    pub pool: PoolConfig,
    pub accounts: Vec<AccountConfig>,
    pub proxy: ProxyConfig,
}
```

- [ ] **Step 4: Run the config tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test cli_config`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/Cargo.toml smelly-connect-cli/src/lib.rs smelly-connect-cli/src/main.rs smelly-connect-cli/src/cli.rs smelly-connect-cli/src/config.rs smelly-connect-cli/tests/cli_config.rs smelly-connect-cli/tests/fixtures/config.sample.toml
git commit -m "feat: add cli parsing and config loading"
```

## Task 3: Build The Account Pool Model With Prewarm, Lazy Connect, And Round-Robin Selection

**Files:**
- Create: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/src/lib.rs`
- Test: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Write failing pool tests**

```rust
#[tokio::test]
async fn pool_prewarms_first_n_accounts() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(4, 2).await;
    assert_eq!(pool.ready_count().await, 2);
}

#[tokio::test]
async fn pool_selects_ready_sessions_round_robin() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["a", "b", "c"]).await;
    assert_eq!(pool.next_account_name().await, "a");
    assert_eq!(pool.next_account_name().await, "b");
    assert_eq!(pool.next_account_name().await, "c");
    assert_eq!(pool.next_account_name().await, "a");
}

#[tokio::test]
async fn pool_lazily_connects_remaining_accounts_on_demand() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(4, 1).await;
    pool.ensure_additional_capacity_for_test().await.unwrap();
    assert!(pool.ready_count().await >= 2);
}

#[tokio::test]
async fn pool_continues_startup_when_some_prewarm_accounts_fail() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_outcomes([Ok("a"), Err("x"), Ok("b")], 3).await;
    assert_eq!(pool.ready_count().await, 2);
}

#[tokio::test]
async fn pool_fails_fast_when_no_ready_sessions_exist() {
    let pool = smelly_connect_cli::pool::SessionPool::from_failed_accounts(2).await;
    let err = pool.next_session().await.unwrap_err();
    assert!(err.to_string().contains("no ready session"));
}

#[tokio::test]
async fn pool_removes_failed_session_from_rotation_and_retries_after_fixed_delay() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_one_failure_for_test().await;
    assert_eq!(pool.ready_count().await, 0);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(pool.ready_count().await >= 1);
}
```

- [ ] **Step 2: Run the pool tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test pool`
Expected: FAIL with missing pool types or behavior

- [ ] **Step 3: Implement pool node states and selection logic**

```rust
pub enum AccountState {
    Configured,
    Connecting,
    Ready(PooledSession),
    Failed(AccountFailure),
}
```

This task must explicitly wire:

- fixed-delay retry using `pool.healthcheck_interval_secs`
- removal of failed nodes from ready rotation
- periodic re-attempt after failure
- fail-fast behavior when no ready sessions exist at selection time

- [ ] **Step 4: Run the pool tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test pool`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/tests/pool.rs
git commit -m "feat: add pooled session selection"
```

## Task 4: Implement `inspect` Commands For Route And Session State

**Files:**
- Modify: `smelly-connect-cli/src/commands/mod.rs`
- Create: `smelly-connect-cli/src/commands/inspect.rs`
- Modify: `smelly-connect-cli/src/main.rs`
- Test: `smelly-connect-cli/tests/inspect.rs`

- [ ] **Step 1: Write failing inspect command tests**

```rust
#[tokio::test]
async fn inspect_route_reports_library_allow_decision() {
    let output = smelly_connect_cli::commands::inspect::inspect_route_for_test("jwxt.sit.edu.cn", 443).await;
    assert!(output.contains("allowed"));
}

#[tokio::test]
async fn inspect_session_reports_pool_summary() {
    let output = smelly_connect_cli::commands::inspect::inspect_session_for_test().await;
    assert!(output.contains("ready="));
}
```

- [ ] **Step 2: Run the inspect tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test inspect`
Expected: FAIL with missing inspect command implementations

- [ ] **Step 3: Implement text-mode inspect commands**

```rust
pub async fn run_route(...) -> Result<(), anyhow::Error> { /* ... */ }
pub async fn run_session(...) -> Result<(), anyhow::Error> { /* ... */ }
```

- [ ] **Step 4: Run the inspect tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test inspect`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/commands/mod.rs smelly-connect-cli/src/commands/inspect.rs smelly-connect-cli/src/main.rs smelly-connect-cli/tests/inspect.rs
git commit -m "feat: add inspect commands"
```

## Task 5: Implement `test tcp|icmp|http` Commands

**Files:**
- Create: `smelly-connect-cli/src/commands/test.rs`
- Modify: `smelly-connect-cli/src/commands/mod.rs`
- Modify: `smelly-connect-cli/src/main.rs`
- Test: `smelly-connect-cli/tests/test_commands.rs`

- [ ] **Step 1: Write failing tests for `test tcp`, `test icmp`, and `test http`**

```rust
#[tokio::test]
async fn test_tcp_reports_success_on_connect() {
    let output = smelly_connect_cli::commands::test::run_tcp_for_test("example.com:443").await.unwrap();
    assert!(output.contains("tcp ok"));
}

#[tokio::test]
async fn test_icmp_uses_session_level_ping() {
    let output = smelly_connect_cli::commands::test::run_icmp_for_test("10.0.0.8").await.unwrap();
    assert!(output.contains("icmp ok"));
}

#[tokio::test]
async fn test_http_fetches_url() {
    let output = smelly_connect_cli::commands::test::run_http_for_test("http://intranet.zju.edu.cn/health").await.unwrap();
    assert!(output.contains("status="));
}
```

- [ ] **Step 2: Run the command tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test test_commands`
Expected: FAIL with missing test command implementations

- [ ] **Step 3: Implement text-mode test commands**

```rust
pub async fn run_tcp(...) -> Result<(), anyhow::Error> { /* ... */ }
pub async fn run_icmp(...) -> Result<(), anyhow::Error> { /* ... */ }
pub async fn run_http(...) -> Result<(), anyhow::Error> { /* ... */ }
```

- [ ] **Step 4: Run the command tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test test_commands`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/commands/test.rs smelly-connect-cli/src/commands/mod.rs smelly-connect-cli/src/main.rs smelly-connect-cli/tests/test_commands.rs
git commit -m "feat: add cli test commands"
```

## Task 6: Implement The HTTP Proxy Service Over The Pool

**Files:**
- Create: `smelly-connect-cli/src/proxy/mod.rs`
- Create: `smelly-connect-cli/src/proxy/http.rs`
- Create: `smelly-connect-cli/src/commands/proxy.rs`
- Modify: `smelly-connect-cli/src/main.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`

- [ ] **Step 1: Write failing HTTP proxy tests**

```rust
#[tokio::test]
async fn http_proxy_uses_pool_and_forwards_requests() {
    let result = smelly_connect_cli::proxy::http::proxy_http_for_test().await.unwrap();
    assert_eq!(result.body, "ok");
    assert_eq!(result.account_name, "acct-01");
    assert!(result.used_pool_selection);
}

#[tokio::test]
async fn http_connect_proxy_tunnels_bytes_through_selected_session() {
    let result = smelly_connect_cli::proxy::http::proxy_connect_for_test().await.unwrap();
    assert_eq!(result.account_name, "acct-01");
    assert_eq!(result.echoed_bytes, b"ping");
}

#[tokio::test]
async fn http_proxy_fails_fast_when_pool_has_no_ready_session() {
    let result = smelly_connect_cli::proxy::http::proxy_http_no_ready_session_for_test().await.unwrap();
    assert_eq!(result.status_code, 503);
}
```

- [ ] **Step 2: Run the HTTP proxy tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test http_proxy`
Expected: FAIL with missing proxy orchestration

- [ ] **Step 3: Implement the pool-backed HTTP proxy command**

```rust
pub async fn serve_http(...) -> Result<(), anyhow::Error> { /* bind a real listener, accept HTTP and CONNECT, select a ready pooled session, and forward real bytes through it */ }
```

- [ ] **Step 4: Run the HTTP proxy tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test http_proxy`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/proxy/mod.rs smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/commands/proxy.rs smelly-connect-cli/src/main.rs smelly-connect-cli/tests/http_proxy.rs
git commit -m "feat: add cli http proxy service"
```

## Task 7: Implement The SOCKS5 Proxy Service Over The Pool

**Files:**
- Create: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/src/commands/proxy.rs`
- Modify: `smelly-connect-cli/src/main.rs`
- Test: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Write the failing SOCKS5 proxy tests**

```rust
#[tokio::test]
async fn socks5_proxy_supports_tcp_connect() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_for_test().await.unwrap();
    assert_eq!(result.account_name, "acct-01");
    assert!(result.used_pool_selection);
    assert_eq!(result.echoed_bytes, b"ping");
}

#[tokio::test]
async fn socks5_proxy_returns_failure_when_no_ready_session_exists() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_no_ready_session_for_test().await.unwrap();
    assert_eq!(result.reply_code, 0x01);
}
```

- [ ] **Step 2: Run the SOCKS5 tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test socks5_proxy`
Expected: FAIL with missing SOCKS5 implementation

- [ ] **Step 3: Implement pool-backed SOCKS5 CONNECT handling**

```rust
pub async fn serve_socks5(...) -> Result<(), anyhow::Error> { /* speak a real SOCKS5 CONNECT handshake on an ephemeral listener, select a ready pooled session, and forward bytes end-to-end */ }
```

- [ ] **Step 4: Run the SOCKS5 tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test socks5_proxy`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/src/commands/proxy.rs smelly-connect-cli/src/main.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "feat: add cli socks5 proxy service"
```

## Task 8: Finalize Foreground `proxy` Command, Docs, And Full Verification

**Files:**
- Modify: `smelly-connect-cli/src/main.rs`
- Modify: `smelly-connect-cli/src/cli.rs`
- Modify: `README.md`
- Modify: `smelly-connect/README.md`
- Modify: `docs/superpowers/specs/2026-03-26-smelly-connect-cli-design.md`

- [ ] **Step 1: Write a failing smoke test for the foreground proxy command**

```rust
#[test]
fn proxy_command_accepts_config_and_listener_overrides() {
    let cli = smelly_connect_cli::cli::Cli::parse_from([
        "smelly-connect-cli",
        "--config",
        "config.toml",
        "proxy",
        "--listen-http",
        "127.0.0.1:8080",
        "--listen-socks5",
        "127.0.0.1:1080",
    ]);
    assert!(matches!(cli.command, smelly_connect_cli::cli::Command::Proxy(_)));
}

#[test]
fn cli_flags_override_config_values() {
    let merged = smelly_connect_cli::config::merge_for_test(
        "tests/fixtures/config.sample.toml",
        ["--prewarm", "5", "--listen-http", "127.0.0.1:18080"],
    )
    .unwrap();
    assert_eq!(merged.pool.prewarm, 5);
    assert_eq!(merged.proxy.http.listen, "127.0.0.1:18080");
}

#[test]
fn explicit_config_path_overrides_default_config_toml_lookup() {
    let merged = smelly_connect_cli::config::load_for_test("tests/fixtures/config.sample.toml").unwrap();
    assert_eq!(merged.accounts.len(), 2);
}
```

- [ ] **Step 2: Run the smoke test to verify it fails**

Run: `cargo test -p smelly-connect-cli --test cli_config proxy_command_accepts_config_and_listener_overrides -- --exact`
Expected: FAIL until the final proxy command shape is wired

- [ ] **Step 3: Finish the foreground command surface, docs, and config precedence behavior**

```rust
// precedence: CLI flags > config.toml > defaults
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
git add smelly-connect-cli/src/main.rs smelly-connect-cli/src/cli.rs README.md smelly-connect/README.md docs/superpowers/specs/2026-03-26-smelly-connect-cli-design.md
git commit -m "feat: finalize smelly-connect-cli"
```

## Notes For The Implementer

- Keep the CLI crate application-only. Do not move pool orchestration into `smelly-connect` unless a new reusable façade abstraction is clearly needed.
- For HTTP proxy serving, it is acceptable to implement the server directly with `hyper` in the CLI crate if that is cleaner than trying to peel the existing library HTTP proxy wrapper apart.
- For SOCKS5 serving, prefer an existing focused SOCKS5 server crate and connect it to pooled `smelly-connect` sessions rather than writing the protocol from scratch unless the crate options are inadequate.
- For `test icmp`, use the library’s session-level ICMP path. Do not add raw OS ping behavior.
- When the pool reaches zero ready sessions during `proxy`, keep the process alive and fail individual requests fast.
- First implementation output is human-readable text only. Do not add JSON output in this plan.
