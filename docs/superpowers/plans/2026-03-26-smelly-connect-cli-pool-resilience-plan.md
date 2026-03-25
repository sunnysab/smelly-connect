# smelly-connect-cli Pool Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `smelly-connect-cli` resilient when account sessions or the school VPN become unavailable by adding threshold-based node ejection, exponential backoff, request-triggered recovery probes, and protocol-correct fast-fail behavior.

**Architecture:** Extend the existing CLI pool with per-node failure counters and explicit node states, keeping request routing as round-robin across selectable nodes (`Ready` and `Suspect`) while removing `Open` and `HalfOpen` nodes from normal rotation. Wire request-time recovery probes and protocol-specific failure replies into the HTTP and SOCKS5 proxy layers without pushing generic circuit-breaker machinery into the `smelly-connect` library crate.

**Tech Stack:** Rust stable, `smelly-connect-cli`, `smelly-connect`, Tokio, existing pool/proxy layers, `cargo test`, `cargo clippy`.

---

## Planned File Structure

### Modify

- `smelly-connect-cli/src/config.rs`
- `smelly-connect-cli/src/pool.rs`
- `smelly-connect-cli/src/proxy/http.rs`
- `smelly-connect-cli/src/proxy/socks5.rs`
- `smelly-connect-cli/tests/pool.rs`
- `smelly-connect-cli/tests/http_proxy.rs`
- `smelly-connect-cli/tests/socks5_proxy.rs`
- `config.toml.example`
- `README.md`
- `docs/superpowers/specs/2026-03-26-smelly-connect-cli-pool-resilience-design.md`

## Task 1: Extend The Test Harness For Resilience Work

**Files:**
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/tests/pool.rs`
- Modify: `smelly-connect-cli/tests/http_proxy.rs`
- Modify: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Write failing tests that require shared resilience helpers**

```rust
#[tokio::test]
async fn pool_exposes_state_summary_and_selectable_count_for_tests() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    assert!(pool.state_summary_for_test().await.contains("Ready"));
    assert!(pool.has_selectable_nodes_for_test().await);
}
```

- [ ] **Step 2: Run the harness tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test pool pool_exposes_state_summary_and_selectable_count_for_tests -- --exact`
Expected: FAIL with missing `_for_test` helpers

- [ ] **Step 3: Add and standardize the resilience test helpers**

```rust
// Examples:
// force_failures_for_test
// state_summary_for_test
// has_selectable_nodes_for_test
// current_backoff_for_test
// force_probe_failure_for_test
// from_exhausted_pool_for_test
// try_request_triggered_probe_for_test
// run_concurrent_probe_race_for_test
```

This task must also update the test proxy harness so it can accept multiple sequential requests, because later liveness tests depend on the listener staying usable across more than one request.

- [ ] **Step 4: Run the harness tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test pool pool_exposes_state_summary_and_selectable_count_for_tests -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/tests/pool.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "test: extend resilience harness"
```

## Task 2: Add Resilience Config Fields And Defaults

**Files:**
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `config.toml.example`
- Test: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Write failing config tests for resilience options**

```rust
#[test]
fn resilience_defaults_are_present() {
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
    assert_eq!(cfg.pool.failure_threshold, 3);
    assert_eq!(cfg.pool.backoff_base_secs, 30);
    assert_eq!(cfg.pool.backoff_max_secs, 600);
    assert!(cfg.pool.allow_request_triggered_probe);
}
```

- [ ] **Step 2: Run the config tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test pool resilience_defaults_are_present -- --exact`
Expected: FAIL with missing resilience fields

- [ ] **Step 3: Add resilience config fields and defaults**

```rust
pub struct PoolConfig {
    pub prewarm: usize,
    pub connect_timeout_secs: u64,
    pub healthcheck_interval_secs: u64,
    pub selection: String,
    pub failure_threshold: u32,
    pub backoff_base_secs: u64,
    pub backoff_max_secs: u64,
    pub allow_request_triggered_probe: bool,
}
```

- [ ] **Step 4: Run the config tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test pool resilience_defaults_are_present -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/config.rs config.toml.example smelly-connect-cli/tests/pool.rs
git commit -m "feat: add pool resilience config"
```

## Task 3: Introduce Explicit Node States And Failure Threshold Handling

**Files:**
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Write failing tests for `Ready -> Suspect -> Open` transitions**

```rust
#[tokio::test]
async fn single_failure_marks_node_suspect_but_keeps_it_selectable() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(1).await;
    assert!(pool.state_summary_for_test().contains("Suspect"));
    assert!(pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn threshold_crossing_moves_node_to_open_and_removes_it_from_rotation() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    assert!(pool.state_summary_for_test().contains("Open"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn normal_selection_uses_ready_and_suspect_but_excludes_open_and_half_open() {
    let pool = smelly_connect_cli::pool::SessionPool::from_mixed_state_pool_for_test().await;
    let picks = pool.collect_selected_accounts_for_test(4).await;
    assert!(picks.iter().all(|name| name == "ready-01" || name == "suspect-01"));
}
```

- [ ] **Step 2: Run the pool tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test pool`
Expected: FAIL with missing node states or threshold handling

- [ ] **Step 3: Add explicit node state transitions**

```rust
pub enum AccountState {
    Configured(AccountConfig),
    Connecting,
    Ready(Box<PooledSession>),
    Suspect(Box<PooledSession>, FailureStats),
    Open(AccountFailure),
    HalfOpen(AccountConfig),
}
```

This task must make the normal selection set explicit in code and tests:

- selectable for normal round-robin = `Ready + Suspect`
- excluded from normal round-robin = `Open + HalfOpen`

This task must explicitly define failure classification:

- count toward consecutive failures:
  - upstream connect timeout
  - upstream connect I/O failure
  - handshake/session invalidation proving the selected upstream is unusable
  - request forwarding failure before upstream establishment completes
- do not count toward consecutive failures:
  - downstream client disconnect after tunnel establishment
  - successful upstream HTTP `5xx`
  - expected cancellation after an established tunnel exists

- [ ] **Step 4: Run the pool tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test pool`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/tests/pool.rs
git commit -m "feat: add threshold-based node state transitions"
```

## Task 4: Implement Exponential Backoff And Timed Re-Entry

**Files:**
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Write failing tests for exponential backoff**

```rust
#[tokio::test]
async fn backoff_grows_exponentially_and_respects_maximum() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    let first = pool.current_backoff_for_test().await;
    pool.force_probe_failure_for_test().await;
    let second = pool.current_backoff_for_test().await;
    assert!(second > first);
    assert!(second <= std::time::Duration::from_secs(600));
}

#[tokio::test(start_paused = true)]
async fn open_node_reenters_via_timer_into_half_open_after_backoff_expiry() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    assert!(pool.state_summary_for_test().await.contains("HalfOpen"));
}
```

- [ ] **Step 2: Run the pool tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test pool backoff_grows_exponentially_and_respects_maximum -- --exact`
Expected: FAIL with missing backoff behavior

- [ ] **Step 3: Add exponential backoff calculation and `open_until` handling**

```rust
fn next_backoff(current: Duration, base: Duration, max: Duration) -> Duration { /* ... */ }
```

This task must use deterministic time control for tests, such as `tokio::time::pause()` / `advance()`, rather than real sleeps.

- [ ] **Step 4: Run the pool tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test pool backoff_grows_exponentially_and_respects_maximum -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/tests/pool.rs
git commit -m "feat: add exponential backoff for failed nodes"
```

## Task 5: Add Request-Triggered Recovery Probe Semantics

**Files:**
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Write failing tests for request-triggered probe behavior**

```rust
#[tokio::test]
async fn request_triggered_probe_recovers_one_node_when_pool_is_exhausted() {
    let pool = smelly_connect_cli::pool::SessionPool::from_exhausted_pool_for_test().await;
    let recovered = pool.try_request_triggered_probe_for_test().await.unwrap();
    assert_eq!(recovered.account_name(), "acct-01");
}

#[tokio::test]
async fn concurrent_requests_do_not_probe_same_node_twice() {
    let pool = smelly_connect_cli::pool::SessionPool::from_exhausted_pool_for_test().await;
    let results = pool.run_concurrent_probe_race_for_test().await;
    assert_eq!(results.successes, 1);
    assert_eq!(results.fast_failures, 1);
}

#[tokio::test]
async fn successful_probe_returns_node_to_ready_and_back_into_normal_rotation() {
    let pool = smelly_connect_cli::pool::SessionPool::from_exhausted_pool_for_test().await;
    let _ = pool.try_request_triggered_probe_for_test().await.unwrap();
    assert!(pool.has_selectable_nodes_for_test().await);
    let picks = pool.collect_selected_accounts_for_test(1).await;
    assert_eq!(picks, vec!["acct-01".to_string()]);
}
```

- [ ] **Step 2: Run the pool tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test pool`
Expected: FAIL with missing half-open probe serialization

- [ ] **Step 3: Implement `HalfOpen` and one-probe-per-node serialization**

```rust
// per-node half-open guard; no concurrent duplicate probes for the same node
```

- [ ] **Step 4: Run the pool tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test pool`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/tests/pool.rs
git commit -m "feat: add request-triggered recovery probes"
```

## Task 6: Implement Protocol-Specific Fast-Fail Replies

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/tests/http_proxy.rs`
- Modify: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Write failing tests for exhausted-pool fast-fail behavior**

```rust
#[tokio::test]
async fn http_returns_503_when_no_selectable_upstream_exists() {
    let result = smelly_connect_cli::proxy::http::proxy_http_no_ready_session_for_test().await.unwrap();
    assert_eq!(result.status_code, 503);
}

#[tokio::test]
async fn socks5_returns_network_unreachable_when_no_selectable_upstream_exists() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_no_ready_session_for_test().await.unwrap();
    assert_eq!(result.reply_code, 0x03);
}
```

- [ ] **Step 2: Run the proxy tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test http_proxy --test socks5_proxy`
Expected: FAIL until proxy fast-fail replies match the resilience contract

- [ ] **Step 3: Make exhausted-pool behavior protocol-correct**

```rust
// HTTP => 503
// SOCKS5 => reply code 0x03 (network unreachable)
```

- [ ] **Step 4: Run the proxy tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test http_proxy --test socks5_proxy`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "feat: add protocol-specific upstream fast-fail replies"
```

## Task 7: Keep The Process Alive During Total Upstream Outage

**Files:**
- Modify: `smelly-connect-cli/src/commands/proxy.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`
- Test: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Write failing liveness tests**

```rust
#[tokio::test]
async fn http_listener_stays_bound_during_total_pool_outage() {
    let result = smelly_connect_cli::proxy::http::proxy_http_no_ready_session_for_test().await.unwrap();
    assert!(result.status_code == 503);
    // second request should still get a valid response path, proving the listener survives
}
```

- [ ] **Step 2: Run the affected tests to verify they fail**

Run: `cargo test -p smelly-connect-cli --test http_proxy --test socks5_proxy`
Expected: FAIL if the service exits or stops binding during total outage

- [ ] **Step 3: Ensure empty-pool periods do not terminate foreground serving**

```rust
// listener stays alive; failures are request-scoped
// add an explicit startup path such as from_config_allow_empty(...) or PoolStartupMode::AllowEmpty
// so long-running proxy mode can opt into empty-start behavior without changing other command semantics
```

- [ ] **Step 4: Run the affected tests to verify they pass**

Run: `cargo test -p smelly-connect-cli --test http_proxy --test socks5_proxy`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/commands/proxy.rs smelly-connect-cli/src/pool.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "feat: keep proxy alive during upstream outages"
```

## Task 8: Update Example Config, Docs, And Full Verification

**Files:**
- Modify: `config.toml.example`
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-03-26-smelly-connect-cli-pool-resilience-design.md`

- [ ] **Step 1: Write a failing config/doc smoke test**

```rust
#[test]
fn config_example_contains_resilience_fields() {
    let body = std::fs::read_to_string("config.toml.example").unwrap();
    assert!(body.contains("failure_threshold"));
    assert!(body.contains("backoff_base_secs"));
    assert!(body.contains("backoff_max_secs"));
    assert!(body.contains("allow_request_triggered_probe"));
}
```

- [ ] **Step 2: Run the smoke test to verify it fails**

Run: `cargo test -p smelly-connect-cli --test pool config_example_contains_resilience_fields -- --exact`
Expected: FAIL until docs/example are updated

- [ ] **Step 3: Update docs and example config**

```toml
[pool]
failure_threshold = 3
backoff_base_secs = 30
backoff_max_secs = 600
allow_request_triggered_probe = true
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
git add config.toml.example README.md docs/superpowers/specs/2026-03-26-smelly-connect-cli-pool-resilience-design.md
git commit -m "feat: document pool resilience"
```

## Notes For The Implementer

- Treat `Ready + Suspect` as the selectable routing set.
- `Open` and `HalfOpen` must not participate in normal round-robin.
- Request-triggered probes must be serialized so one node cannot be probed by multiple concurrent requests at once.
- HTTP fast-fail is `503`; SOCKS5 fast-fail is `0x03` (`network unreachable`).
- The proxy process must remain alive during total upstream outage; request failure is not process failure.
