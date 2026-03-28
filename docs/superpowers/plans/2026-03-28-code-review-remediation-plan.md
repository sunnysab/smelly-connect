# Code Review Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the reviewed correctness, performance, and boundary issues from the workspace while keeping the current EasyConnect and proxy behavior verifiable at every checkpoint.

**Architecture:** First fix lifecycle and correctness bugs that can leak resources or misreport health. Then make route semantics and hot-path data structures honest. Finally shrink API surface, isolate test-only code, and clean up remaining protocol/transport hazards. Each task is independently shippable and ends with targeted verification plus a small commit.

**Tech Stack:** Rust, Tokio, Hyper, fast-socks5, reqwest, smoltcp, OpenSSL, Cargo test/clippy

---

## File Structure

**Proxy lifecycle and reqwest integration**
- Modify: `smelly-connect/src/integration/reqwest.rs`
  Responsibility: keep any internal proxy listener alive only as long as the returned client wrapper lives
- Modify: `smelly-connect/src/proxy/http.rs`
  Responsibility: make proxy handle cleanup automatic and explicit
- Modify: `smelly-connect/src/session.rs`
  Responsibility: expose the owned reqwest integration from the session API
- Modify: `smelly-connect/tests/reqwest_integration.rs`
  Responsibility: regression coverage for proxy-handle lifetime

**CLI runtime, supervision, and backpressure**
- Modify: `smelly-connect-cli/src/main.rs`
  Responsibility: use the right Tokio runtime for long-lived proxy work
- Modify: `smelly-connect-cli/src/commands/proxy.rs`
  Responsibility: supervise listener tasks and fail loudly when one dies
- Modify: `smelly-connect-cli/src/proxy/http.rs`
  Responsibility: add bounded connection admission
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
  Responsibility: add bounded connection admission
- Modify: `smelly-connect-cli/tests/proxy_command.rs`
  Responsibility: assert listener failure surfaces correctly

**Route semantics**
- Create: `smelly-connect/src/domain/route_protocol.rs`
  Responsibility: typed route protocol model
- Create: `smelly-connect/src/domain/route_match.rs`
  Responsibility: shared domain/IP route matching helpers without hot-path allocation
- Modify: `smelly-connect/src/domain/mod.rs`
- Modify: `smelly-connect/src/resource/model.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect/tests/routing.rs`
- Modify: `smelly-connect-cli/tests/cli_config.rs`

**Runtime health accounting**
- Modify: `smelly-connect-cli/src/runtime.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/tests/status_command.rs`
- Modify: `smelly-connect-cli/tests/logging_runtime.rs`

**Session and pool hot paths**
- Create: `smelly-connect/src/session/inner.rs`
  Responsibility: cheap-clone shared session state
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/resolver/mod.rs`
- Modify: `smelly-connect/src/resource/model.rs`
- Create: `smelly-connect-cli/src/pool/state.rs`
  Responsibility: compact node storage and mutation helpers
- Create: `smelly-connect-cli/src/pool/selection.rs`
  Responsibility: selection and probe candidate logic
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/tests/pool.rs`

**HTTP throughput**
- Create: `smelly-connect-cli/src/proxy/http/upstream.rs`
  Responsibility: reusable upstream connection/session helpers
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/tests/http_proxy.rs`

**Boundary cleanup and test isolation**
- Modify: `smelly-connect/src/lib.rs`
- Modify: `smelly-connect/src/auth/mod.rs`
- Modify: `smelly-connect/src/protocol/mod.rs`
- Modify: `smelly-connect/src/runtime/mod.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/src/runtime.rs`
- Create: `smelly-connect-cli/src/proxy/http/test_harness.rs`
- Create: `smelly-connect-cli/src/proxy/socks5/test_harness.rs`
- Create: `smelly-connect-cli/src/runtime/test_support.rs`
- Modify: `smelly-connect/tests/public_api_audit.rs`
- Create: `smelly-connect/tests/ui/public_surface.rs`

**Error and protocol hygiene**
- Create: `smelly-connect-cli/src/error.rs`
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect-cli/src/logging.rs`
- Modify: `smelly-connect-cli/src/commands/inspect.rs`
- Modify: `smelly-connect-cli/src/commands/proxy.rs`
- Modify: `smelly-connect-cli/src/commands/routes.rs`
- Modify: `smelly-connect-cli/src/commands/status.rs`
- Modify: `smelly-connect-cli/src/commands/test.rs`
- Modify: `smelly-connect/src/error.rs`
- Modify: `smelly-connect/src/transport/netstack.rs`
- Modify: `smelly-tls/src/lib.rs`
- Modify: `smelly-connect-cli/tests/cli_config.rs`
- Modify: `smelly-connect/tests/connect_control_plane.rs`
- Modify: `smelly-connect/tests/transport_stack.rs`

---

### Task 1: Fix Reqwest Proxy Lifetime And Automatic ProxyHandle Cleanup

**Files:**
- Modify: `smelly-connect/src/integration/reqwest.rs`
- Modify: `smelly-connect/src/proxy/http.rs`
- Modify: `smelly-connect/src/session.rs`
- Test: `smelly-connect/tests/reqwest_integration.rs`

- [ ] **Step 1: Write the failing test**

Add a regression test that builds a reqwest client through a session, confirms the internal proxy accepts traffic while the client is alive, then drops the wrapper and confirms the listener is gone.

```rust
let (client, addr) = smelly_connect::integration::reqwest::build_client_for_test(&session).await?;
assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
drop(client);
tokio::time::sleep(Duration::from_millis(20)).await;
assert!(tokio::net::TcpStream::connect(addr).await.is_err());
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p smelly-connect reqwest_integration -- --nocapture`
Expected: FAIL because the proxy listener still accepts connections after the reqwest client is dropped.

- [ ] **Step 3: Introduce an owned reqwest wrapper**

Keep the proxy handle alive inside a small owned wrapper rather than forgetting it.

```rust
pub struct SessionReqwestClient {
    client: reqwest::Client,
    _proxy: ProxyHandle,
}
```

- [ ] **Step 4: Make `ProxyHandle` clean up on drop**

Store the accept-loop join handle and abort or signal it from `Drop`.

```rust
impl Drop for ProxyHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
```

- [ ] **Step 5: Keep `Session::reqwest_client()` ergonomic**

Either return the wrapper directly or keep `reqwest::Client` API compatibility by ensuring the reqwest proxy closure retains the cleanup guard.

- [ ] **Step 6: Run targeted tests**

Run: `cargo test -p smelly-connect reqwest_integration lifecycle_handles -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add smelly-connect/src/integration/reqwest.rs smelly-connect/src/proxy/http.rs smelly-connect/src/session.rs smelly-connect/tests/reqwest_integration.rs
git commit -m "fix: tie reqwest proxy lifetime to client ownership"
```

### Task 2: Supervise CLI Listeners And Move To A Multi-Thread Runtime

**Files:**
- Modify: `smelly-connect-cli/src/main.rs`
- Modify: `smelly-connect-cli/src/commands/proxy.rs`
- Test: `smelly-connect-cli/tests/proxy_command.rs`

- [ ] **Step 1: Write the failing regression test**

Add a test that starts proxy mode with one intentionally failing listener and asserts the command returns an error instead of hanging behind the first successful listener.

```rust
let err = smelly_connect_cli::commands::proxy::run_proxy(config_path, &command).await.unwrap_err();
assert!(err.contains("listener"));
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p smelly-connect-cli proxy_command -- --nocapture`
Expected: FAIL because listener task failure is hidden behind ordered `JoinHandle` awaiting.

- [ ] **Step 3: Switch the CLI runtime to multi-thread**

Replace the current-thread runtime with multi-thread plus worker threads.

```rust
let rt = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("build runtime");
```

- [ ] **Step 4: Replace ordered task awaiting with supervision**

Collect listener futures with `JoinSet` or `tokio::select!` so any listener failure tears down the command.

```rust
while let Some(result) = join_set.join_next().await {
    result.map_err(|err| err.to_string())??;
}
```

- [ ] **Step 5: Run targeted tests**

Run: `cargo test -p smelly-connect-cli proxy_command status_command routes_command -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect-cli/src/main.rs smelly-connect-cli/src/commands/proxy.rs smelly-connect-cli/tests/proxy_command.rs
git commit -m "fix: supervise proxy listeners on a multithread runtime"
```

### Task 3: Add Listener Backpressure Instead Of Unbounded Spawn

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`
- Test: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Write the failing tests**

Add tests that open more concurrent client sockets than the configured cap and assert extra requests are rejected quickly instead of waiting forever.

```rust
assert_eq!(status_code, 503);
assert_eq!(reply_code, 0x03);
```

- [ ] **Step 2: Run targeted tests to verify failure**

Run: `cargo test -p smelly-connect-cli http_proxy socks5_proxy -- --nocapture`
Expected: FAIL because there is no admission control.

- [ ] **Step 3: Introduce a small connection limiter**

Use a `tokio::sync::Semaphore` shared by each listener.

```rust
let permit = limiter.clone().try_acquire_owned().ok();
```

- [ ] **Step 4: Return protocol-correct overload responses**

HTTP should return `503 Service Unavailable`.
SOCKS5 should return `network unreachable` or a dedicated temporary failure code if the library exposes one.

- [ ] **Step 5: Run targeted tests**

Run: `cargo test -p smelly-connect-cli http_proxy socks5_proxy -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "feat: add proxy listener admission control"
```

### Task 4: Make Route Protocol Semantics Real And Shared

**Files:**
- Create: `smelly-connect/src/domain/route_protocol.rs`
- Create: `smelly-connect/src/domain/route_match.rs`
- Modify: `smelly-connect/src/domain/mod.rs`
- Modify: `smelly-connect/src/resource/model.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Test: `smelly-connect/tests/routing.rs`
- Test: `smelly-connect-cli/tests/cli_config.rs`

- [ ] **Step 1: Write the failing tests**

Add library tests proving a TCP-only rule does not match UDP and vice versa, plus config tests proving invalid protocol strings are rejected.

```rust
assert!(!session.allows_udp(("foo.edu.cn", 53)));
assert!(load(path).is_err());
```

- [ ] **Step 2: Run targeted tests to verify failure**

Run: `cargo test -p smelly-connect routing -- --nocapture`
Run: `cargo test -p smelly-connect-cli cli_config -- --nocapture`
Expected: FAIL because protocol strings are currently ignored during route matching.

- [ ] **Step 3: Introduce a typed route protocol**

```rust
pub enum RouteProtocol {
    Tcp,
    Udp,
    All,
}
```

- [ ] **Step 4: Move match logic into one shared helper**

```rust
pub fn domain_rule_matches(host: &str, port: u16, protocol: RouteProtocol, rule: &DomainRule) -> bool
```

- [ ] **Step 5: Remove hot-path `format!` allocation**

Precompute suffix matching using slices instead of `format!(".{domain}")`.

- [ ] **Step 6: Run targeted tests**

Run: `cargo test -p smelly-connect routing -- --nocapture`
Run: `cargo test -p smelly-connect-cli cli_config -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add smelly-connect/src/domain/route_protocol.rs smelly-connect/src/domain/route_match.rs smelly-connect/src/domain/mod.rs smelly-connect/src/resource/model.rs smelly-connect/src/session.rs smelly-connect-cli/src/config.rs smelly-connect-cli/src/pool.rs smelly-connect/tests/routing.rs smelly-connect-cli/tests/cli_config.rs
git commit -m "fix: honor protocol-aware route rules"
```

### Task 5: Wire Runtime Connect Failure Accounting

**Files:**
- Modify: `smelly-connect-cli/src/runtime.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Test: `smelly-connect-cli/tests/status_command.rs`
- Test: `smelly-connect-cli/tests/logging_runtime.rs`

- [ ] **Step 1: Write the failing test**

Add a status/runtime test that forces an upstream connect failure and asserts the effective health becomes `recovering` until the next successful connect.

```rust
assert_eq!(snapshot.status, PoolHealthStatus::Recovering);
```

- [ ] **Step 2: Run targeted test to verify failure**

Run: `cargo test -p smelly-connect-cli status_command logging_runtime -- --nocapture`
Expected: FAIL because production code never records connect failures.

- [ ] **Step 3: Classify failure points explicitly**

Call `stats.record_connect_failure()` from HTTP and SOCKS5 connect failure branches that represent meaningful upstream/session failure.

- [ ] **Step 4: Keep route rejection separate**

Do not record route-policy rejection as a transport failure.

- [ ] **Step 5: Run targeted tests**

Run: `cargo test -p smelly-connect-cli status_command logging_runtime http_proxy socks5_proxy -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect-cli/src/runtime.rs smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/tests/status_command.rs smelly-connect-cli/tests/logging_runtime.rs
git commit -m "fix: record runtime connect failures"
```

### Task 6: Make `Session` Clone Cheap And Remove Deep-Copy Hot Paths

**Files:**
- Create: `smelly-connect/src/session/inner.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/resolver/mod.rs`
- Modify: `smelly-connect/src/resource/model.rs`
- Test: `smelly-connect/tests/routing.rs`
- Test: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Write the failing characterization test**

Add a test or benchmark-style assertion that repeated `Session` clones share the same inner allocation instead of duplicating route/DNS structures.

```rust
assert!(std::ptr::eq(session.inner_ptr(), clone.inner_ptr()));
```

- [ ] **Step 2: Run targeted test to verify failure**

Run: `cargo test -p smelly-connect routing -- --nocapture`
Run: `cargo test -p smelly-connect-cli pool -- --nocapture`
Expected: FAIL because `EasyConnectSession` is still a deep clone.

- [ ] **Step 3: Introduce an `Arc`-backed inner session state**

```rust
struct SessionInner {
    client_ip: Ipv4Addr,
    resources: ResourceSet,
    resolver: SessionResolver,
    transport: TransportStack,
    legacy_data_plane: Option<LegacyDataPlaneConfig>,
    runtime: Arc<SessionRuntime>,
}
```

- [ ] **Step 4: Keep builder-style mutation ergonomic**

For `with_local_route_overrides` and `with_allow_all_routes`, decide whether they mutate lightweight outer config or use `Arc::make_mut`.

- [ ] **Step 5: Run targeted tests**

Run: `cargo test -p smelly-connect routing reqwest_integration lifecycle_handles -- --nocapture`
Run: `cargo test -p smelly-connect-cli pool -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect/src/session/inner.rs smelly-connect/src/session.rs smelly-connect/src/resolver/mod.rs smelly-connect/src/resource/model.rs smelly-connect/tests/routing.rs smelly-connect-cli/tests/pool.rs
git commit -m "refactor: make session clones cheap"
```

### Task 7: Split Pool State And Remove Repeated Whole-State Work

**Files:**
- Create: `smelly-connect-cli/src/pool/state.rs`
- Create: `smelly-connect-cli/src/pool/selection.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Test: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Write the failing tests**

Add regression tests for:
- request-triggered probe choosing only one node at a time
- half-open timer transitions
- round-robin behavior after refactor

```rust
assert_eq!(result.successes, 1);
assert_eq!(names, vec!["acct-01", "acct-02", "acct-01"]);
```

- [ ] **Step 2: Run targeted tests to verify the safety net**

Run: `cargo test -p smelly-connect-cli pool -- --nocapture`
Expected: PASS before refactor; keep this as characterization coverage.

- [ ] **Step 3: Move node storage helpers out of the monolith**

Extract state mutation helpers and selection logic into focused modules.

```rust
pub(crate) fn next_selectable_index(...)
pub(crate) fn refresh_time_based_states(...)
```

- [ ] **Step 4: Remove duplicated account/session fields**

Keep account identity in one place and stop storing the same name/config in three layers.

- [ ] **Step 5: Add bounded concurrent prewarm and probe fan-out**

Use `JoinSet` or `buffer_unordered` with a small cap instead of strict serial loops.

- [ ] **Step 6: Re-run pool tests**

Run: `cargo test -p smelly-connect-cli pool -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/src/pool/state.rs smelly-connect-cli/src/pool/selection.rs smelly-connect-cli/tests/pool.rs
git commit -m "refactor: split pool state and selection logic"
```

### Task 8: Enable Safe HTTP Upstream Keep-Alive

**Files:**
- Create: `smelly-connect-cli/src/proxy/http/upstream.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`

- [ ] **Step 1: Write the failing tests**

Add a regression test that sends two sequential origin-form requests through the same proxy/session and asserts a single upstream TCP connection can serve both when the upstream keeps the connection alive.

```rust
assert_eq!(upstream_accept_count.load(Ordering::SeqCst), 1);
```

- [ ] **Step 2: Run targeted test to verify failure**

Run: `cargo test -p smelly-connect-cli http_proxy -- --nocapture`
Expected: FAIL because the proxy currently forces `Connection: close`.

- [ ] **Step 3: Extract reusable upstream connection logic**

Separate request parsing/forwarding from upstream socket ownership.

```rust
struct ReusableUpstream {
    stream: VpnStream,
    host: String,
    port: u16,
}
```

- [ ] **Step 4: Only force close when protocol constraints require it**

Keep `CONNECT` behavior unchanged and preserve chunked/body correctness.

- [ ] **Step 5: Run targeted tests**

Run: `cargo test -p smelly-connect-cli http_proxy -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/http/upstream.rs smelly-connect-cli/tests/http_proxy.rs
git commit -m "perf: reuse upstream http connections when safe"
```

### Task 9: Shrink Public API Surface And Remove Test-Only Debug Pollution

**Files:**
- Modify: `smelly-connect/src/lib.rs`
- Modify: `smelly-connect/src/auth/mod.rs`
- Modify: `smelly-connect/src/protocol/mod.rs`
- Modify: `smelly-connect/src/runtime/mod.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/src/runtime.rs`
- Create: `smelly-connect-cli/src/proxy/http/test_harness.rs`
- Create: `smelly-connect-cli/src/proxy/socks5/test_harness.rs`
- Create: `smelly-connect-cli/src/runtime/test_support.rs`
- Modify: `smelly-connect/tests/public_api_audit.rs`
- Create: `smelly-connect/tests/ui/public_surface.rs`

- [ ] **Step 1: Write the failing API and build-surface tests**

Add trybuild or compile-fail coverage for symbols that should remain private, and add a small check that debug builds no longer expose test-only modules through ordinary compilation.

```rust
let t = trybuild::TestCases::new();
t.compile_fail("tests/ui/public_surface.rs");
```

- [ ] **Step 2: Run targeted tests to verify current leakage**

Run: `cargo test -p smelly-connect public_api_audit -- --nocapture`
Expected: FAIL after adding new compile-fail assertions because internal modules are still publicly reachable.

- [ ] **Step 3: Replace `debug_assertions` gating with test-only support modules**

Move `*_for_test` helpers into dedicated `#[cfg(test)]` or explicit `test_support` modules.

- [ ] **Step 4: Narrow `lib.rs` exports to intentional surface area**

Keep the facade and intentionally supported types public; downgrade internal modules to `pub(crate)` or remove public exposure.

- [ ] **Step 5: Run targeted tests**

Run: `cargo test -p smelly-connect public_api_audit -- --nocapture`
Run: `cargo test -p smelly-connect-cli http_proxy socks5_proxy logging_runtime -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect/src/lib.rs smelly-connect/src/auth/mod.rs smelly-connect/src/protocol/mod.rs smelly-connect/src/runtime/mod.rs smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/src/runtime.rs smelly-connect-cli/src/proxy/http/test_harness.rs smelly-connect-cli/src/proxy/socks5/test_harness.rs smelly-connect-cli/src/runtime/test_support.rs smelly-connect/tests/public_api_audit.rs smelly-connect/tests/ui/public_surface.rs
git commit -m "refactor: isolate test support and narrow public api"
```

### Task 10: Replace Stringly CLI Errors And Remove Production Panic Hotspots

**Files:**
- Create: `smelly-connect-cli/src/error.rs`
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect-cli/src/logging.rs`
- Modify: `smelly-connect-cli/src/commands/inspect.rs`
- Modify: `smelly-connect-cli/src/commands/proxy.rs`
- Modify: `smelly-connect-cli/src/commands/routes.rs`
- Modify: `smelly-connect-cli/src/commands/status.rs`
- Modify: `smelly-connect-cli/src/commands/test.rs`
- Modify: `smelly-connect/src/error.rs`
- Modify: `smelly-connect/src/transport/netstack.rs`
- Modify: `smelly-tls/src/lib.rs`
- Test: `smelly-connect-cli/tests/cli_config.rs`
- Test: `smelly-connect/tests/connect_control_plane.rs`
- Test: `smelly-connect/tests/transport_stack.rs`
- Test: `smelly-tls/tests/record_layer.rs`

- [ ] **Step 1: Write the failing tests**

Add tests that assert invalid configuration/protocol conditions return structured errors rather than panicking.

```rust
assert!(matches!(err, CliError::Config(_)));
```

- [ ] **Step 2: Run targeted tests to verify failure**

Run: `cargo test -p smelly-connect-cli cli_config -- --nocapture`
Run: `cargo test -p smelly-connect connect_control_plane transport_stack -- --nocapture`
Run: `cargo test -p smelly-tls record_layer -- --nocapture`
Expected: FAIL after introducing new assertions because the current code still panics or flattens errors to `String`.

- [ ] **Step 3: Add a typed CLI error boundary**

```rust
pub enum CliError {
    Config(String),
    Logging(String),
    Command(String),
}
```

- [ ] **Step 4: Replace realistic production `unwrap`/`expect` calls**

Convert protocol-stack and netstack assumptions into `Result`-returning code where failure can occur from bad input or invariant drift.

- [ ] **Step 5: Tighten misleading names**

Rename or document IPv4-only resolver helpers so the API matches behavior.

- [ ] **Step 6: Run targeted tests**

Run: `cargo test -p smelly-connect-cli cli_config -- --nocapture`
Run: `cargo test -p smelly-connect connect_control_plane transport_stack -- --nocapture`
Run: `cargo test -p smelly-tls record_layer -- --nocapture`
Expected: PASS

- [ ] **Step 7: Run full verification**

Run: `cargo test --workspace`
Expected: PASS

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add smelly-connect-cli/src/error.rs smelly-connect-cli/src/config.rs smelly-connect-cli/src/logging.rs smelly-connect-cli/src/commands/inspect.rs smelly-connect-cli/src/commands/proxy.rs smelly-connect-cli/src/commands/routes.rs smelly-connect-cli/src/commands/status.rs smelly-connect-cli/src/commands/test.rs smelly-connect/src/error.rs smelly-connect/src/transport/netstack.rs smelly-tls/src/lib.rs smelly-connect-cli/tests/cli_config.rs smelly-connect/tests/connect_control_plane.rs smelly-connect/tests/transport_stack.rs smelly-tls/tests/record_layer.rs
git commit -m "refactor: use structured cli errors and remove panic hotspots"
```

## Notes For The Implementer

- Do not combine Tasks 1-4 into one commit. They touch overlapping files but solve different classes of bugs.
- If Task 8 expands beyond one focused session, split keep-alive transport reuse from request parsing cleanup into a follow-up plan.
- If public API narrowing breaks downstream examples, update README and examples in the same task before merging.
