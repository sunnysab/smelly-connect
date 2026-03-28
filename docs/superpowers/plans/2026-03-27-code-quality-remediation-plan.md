# Code Quality Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the highest-risk correctness and maintainability issues identified in review without destabilizing the current proxy/session behavior.

**Architecture:** First tighten semantics and resource ownership so failures, timeouts, and session lifecycle are modeled correctly. Then split oversized modules and remove test-only leakage from production APIs. Finish by simplifying hot paths and deleting dead configuration or nominal-only layers that do not enforce useful boundaries.

**Tech Stack:** Rust, Tokio, Hyper, fast-socks5, smoltcp, OpenSSL, Cargo test/clippy

---

## File Structure

**Session lifecycle and bootstrap**
- Modify: `smelly-connect/src/config.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/runtime/tasks/keepalive.rs`
- Modify: `smelly-connect/src/auth/control.rs`
- Create: `smelly-connect/src/session/runtime.rs`

**Structured error model**
- Modify: `smelly-connect/src/error.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/src/pool.rs`

**Proxy hardening**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect/src/proxy/http.rs`
- Create: `smelly-connect-cli/src/proxy/http/header_parse.rs`
- Create: `smelly-connect/src/proxy/http/header_parse.rs`

**Pool performance**
- Modify: `smelly-connect-cli/src/pool.rs`
- Create: `smelly-connect-cli/src/pool/selection.rs`
- Create: `smelly-connect-cli/src/pool/state.rs`

**Boundary cleanup**
- Modify: `smelly-connect/src/lib.rs`
- Modify: `smelly-connect/src/domain/mod.rs`
- Delete or collapse: `smelly-connect/src/domain/session.rs`
- Delete or collapse: `smelly-connect/src/domain/resolver.rs`
- Delete or collapse: `smelly-connect/src/domain/stream.rs`
- Delete or collapse: `smelly-connect/src/domain/connect_plan.rs`
- Modify: `smelly-connect-cli/src/config.rs`

**Test support isolation**
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/resolver/mod.rs`
- Modify: `smelly-connect/src/transport/mod.rs`
- Modify: `smelly-connect/src/proxy/mod.rs`
- Modify: `smelly-connect/src/auth/mod.rs`
- Modify: `smelly-connect/src/integration/mod.rs`
- Create: `smelly-connect/src/test_support/session.rs`
- Create: `smelly-connect/src/test_support/resolver.rs`
- Create: `smelly-connect/src/test_support/proxy.rs`
- Create: `smelly-connect/src/test_support/integration.rs`

**Oversized file splits**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-tls/src/lib.rs`
- Create: `smelly-connect-cli/src/proxy/http/live.rs`
- Create: `smelly-connect-cli/src/proxy/http/test_harness.rs`
- Create: `smelly-connect-cli/src/proxy/http/body.rs`
- Create: `smelly-connect-cli/src/proxy/socks5/live.rs`
- Create: `smelly-connect-cli/src/proxy/socks5/test_harness.rs`
- Create: `smelly-connect-cli/src/pool/test_harness.rs`
- Create: `smelly-tls/src/handshake.rs`
- Create: `smelly-tls/src/parser.rs`
- Create: `smelly-tls/src/crypto.rs`

---

### Task 1: Replace Stringly-Typed Transport Errors With Structured Variants

**Files:**
- Modify: `smelly-connect/src/error.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`
- Test: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Write the failing tests**

Add assertions that route rejection, timeout, and generic transport failure map differently in HTTP and SOCKS5.

```rust
assert_eq!(status_code, 403);
assert_eq!(reply_code, expected_reply_code);
```

- [ ] **Step 2: Run targeted tests to verify failure**

Run: `cargo test -p smelly-connect-cli http_proxy socks5_proxy -- --nocapture`
Expected: FAIL because route rejection and timeout are still flattened into generic failures.

- [ ] **Step 3: Introduce structured transport/bootstrap variants**

Refactor `TransportError` and `TunnelBootstrapError` away from single `String` payloads.

```rust
pub enum TransportError {
    ConnectTimedOut,
    ConnectFailed(String),
    ConnectionClosed,
}
```

- [ ] **Step 4: Update HTTP/SOCKS5 mapping logic to match variants instead of parsing strings**

Replace:

```rust
message.to_ascii_lowercase().contains("timed out")
```

With direct enum matching.

- [ ] **Step 5: Run targeted tests**

Run: `cargo test -p smelly-connect-cli http_proxy socks5_proxy -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect/src/error.rs smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "refactor: use structured transport errors"
```

### Task 2: Decouple Route Policy Rejection From Session Health

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`
- Test: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Add failing regression tests**

Add tests proving a route rejection does not move a healthy node to open/unhealthy state.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p smelly-connect-cli route_rejection -- --nocapture`
Expected: FAIL because route rejection currently triggers unhealthy reporting.

- [ ] **Step 3: Split connect result classification**

Introduce a small internal enum near proxy call sites:

```rust
enum ConnectDecision {
    Connected(VpnStream),
    RouteRejected,
    Failed(UpstreamConnectError),
}
```

- [ ] **Step 4: Only call pool health reporting on real transport/session failures**

Keep route rejection on the request path only; do not mutate pool health.

- [ ] **Step 5: Return client-facing policy-appropriate errors**

HTTP should return `403 Forbidden` for route rejection.
SOCKS5 should return `ConnectionNotAllowedByRuleset` if available from `fast-socks5`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p smelly-connect-cli http_proxy socks5_proxy -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/src/pool.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "fix: separate route rejection from session health"
```

### Task 3: Tie Session-Owned Background Work To Session Lifetime

**Files:**
- Modify: `smelly-connect/src/config.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/runtime/tasks/keepalive.rs`
- Create: `smelly-connect/src/session/runtime.rs`
- Test: `smelly-connect/tests/lifecycle_handles.rs`

- [ ] **Step 1: Add failing lifecycle test**

Add a test that drops a session and verifies keepalive/tunnel-holder tasks are shut down.

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p smelly-connect lifecycle_handles -- --nocapture`
Expected: FAIL because detached tasks outlive the session.

- [ ] **Step 3: Introduce session-owned runtime resources**

Move background handles into a dedicated struct stored by `EasyConnectSession`.

```rust
struct SessionRuntime {
    tunnel_hold_task: Option<JoinHandle<()>>,
    keepalive: Option<KeepaliveHandle>,
}
```

- [ ] **Step 4: Replace detached `tokio::spawn` calls with owned handles**

Remove the `pending::<()>()` holder task pattern from bootstrap.

- [ ] **Step 5: Implement cleanup on drop**

Ensure runtime resources are cancelled or shut down when the owning session is dropped.

- [ ] **Step 6: Run test**

Run: `cargo test -p smelly-connect lifecycle_handles -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add smelly-connect/src/config.rs smelly-connect/src/session.rs smelly-connect/src/runtime/tasks/keepalive.rs smelly-connect/src/session/runtime.rs smelly-connect/tests/lifecycle_handles.rs
git commit -m "fix: bind session tasks to session lifetime"
```

### Task 4: Remove Blocking Bootstrap I/O From The Async Connect Path

**Files:**
- Modify: `smelly-connect/src/auth/control.rs`
- Modify: `smelly-connect/src/config.rs`
- Test: `smelly-connect/tests/connect_control_plane.rs`

- [ ] **Step 1: Add a failing concurrency-oriented test or benchmark-style assertion**

Prefer a test that verifies bootstrap work uses `spawn_blocking` or an async equivalent instead of running sync I/O inline.

- [ ] **Step 2: Run test**

Run: `cargo test -p smelly-connect connect_control_plane -- --nocapture`
Expected: FAIL or require implementation changes.

- [ ] **Step 3: Wrap blocking OpenSSL/TCP path**

Move blocking token fetch into:

```rust
tokio::task::spawn_blocking(move || request_token_blocking(&server, &twfid)).await?
```

- [ ] **Step 4: Keep error semantics structured**

Do not regress back to string parsing while moving work off the runtime thread.

- [ ] **Step 5: Run tests**

Run: `cargo test -p smelly-connect connect_control_plane -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect/src/auth/control.rs smelly-connect/src/config.rs smelly-connect/tests/connect_control_plane.rs
git commit -m "perf: move blocking bootstrap io off async workers"
```

### Task 5: Add Explicit Header Size Limits To Both HTTP Proxy Implementations

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect/src/proxy/http.rs`
- Create: `smelly-connect-cli/src/proxy/http/header_parse.rs`
- Create: `smelly-connect/src/proxy/http/header_parse.rs`
- Test: `smelly-connect-cli/tests/http_proxy.rs`
- Test: `smelly-connect/tests/http_proxy.rs`

- [ ] **Step 1: Add failing oversized-header tests**

Write tests that send a header block larger than the chosen cap and assert rejection.

- [ ] **Step 2: Run tests**

Run: `cargo test -p smelly-connect-cli http_proxy -p smelly-connect http_proxy -- --nocapture`
Expected: FAIL because header reads are currently unbounded.

- [ ] **Step 3: Implement bounded header parsing helper**

```rust
const MAX_HEADER_BYTES: usize = 16 * 1024;
```

Reject once the buffer exceeds the cap before the terminator is found.

- [ ] **Step 4: Return deterministic errors**

HTTP CLI proxy should return `431` or `400`; library proxy should return `io::ErrorKind::InvalidData`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p smelly-connect-cli http_proxy -p smelly-connect http_proxy -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect-cli/src/proxy/http.rs smelly-connect/src/proxy/http.rs smelly-connect-cli/src/proxy/http/header_parse.rs smelly-connect/src/proxy/http/header_parse.rs smelly-connect-cli/tests/http_proxy.rs smelly-connect/tests/http_proxy.rs
git commit -m "fix: bound http header parsing"
```

### Task 6: Remove O(n) Session Selection From The Hot Path

**Files:**
- Modify: `smelly-connect-cli/src/pool.rs`
- Create: `smelly-connect-cli/src/pool/selection.rs`
- Create: `smelly-connect-cli/src/pool/state.rs`
- Test: `smelly-connect-cli/tests/pool.rs`

- [ ] **Step 1: Add a failing behavior-preserving test**

Keep round-robin behavior while changing implementation. Add assertions around ready/suspect exclusion rules and cursor progression.

- [ ] **Step 2: Run tests**

Run: `cargo test -p smelly-connect-cli pool -- --nocapture`
Expected: PASS before refactor; record as safety baseline.

- [ ] **Step 3: Refactor pool internals**

Store reusable selectable indices or a compact ready ring in `PoolState`, and update incrementally on state transitions instead of rebuilding `Vec`s per request.

- [ ] **Step 4: Remove avoidable cloning on selection**

Return references/index-selected state internally, cloning only at the API boundary if unavoidable.

- [ ] **Step 5: Run tests**

Run: `cargo test -p smelly-connect-cli pool -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/src/pool/selection.rs smelly-connect-cli/src/pool/state.rs smelly-connect-cli/tests/pool.rs
git commit -m "perf: remove linear pool selection on request path"
```

### Task 7: Move Test Harness Code Out Of Production Modules

**Files:**
- Modify: `smelly-connect/src/lib.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/resolver/mod.rs`
- Modify: `smelly-connect/src/transport/mod.rs`
- Modify: `smelly-connect/src/proxy/mod.rs`
- Modify: `smelly-connect/src/auth/mod.rs`
- Modify: `smelly-connect/src/integration/mod.rs`
- Create: `smelly-connect/src/test_support/session.rs`
- Create: `smelly-connect/src/test_support/resolver.rs`
- Create: `smelly-connect/src/test_support/proxy.rs`
- Create: `smelly-connect/src/test_support/integration.rs`

- [ ] **Step 1: Move one harness at a time**

Start with `session.rs` helpers, then `resolver`, `transport`, `proxy`, `auth`, `integration`.

- [ ] **Step 2: Gate test support appropriately**

Use `#[cfg(test)]` where possible, and avoid `cfg(any(test, debug_assertions))` for production modules.

- [ ] **Step 3: Update CLI test/inspect commands**

If these commands require harness behavior, move them behind explicit non-production test commands or replace them with real config-backed flows only.

- [ ] **Step 4: Run full workspace tests**

Run: `cargo test --workspace --quiet`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/lib.rs smelly-connect/src/session.rs smelly-connect/src/resolver/mod.rs smelly-connect/src/transport/mod.rs smelly-connect/src/proxy/mod.rs smelly-connect/src/auth/mod.rs smelly-connect/src/integration/mod.rs smelly-connect/src/test_support
git commit -m "refactor: isolate test support from production modules"
```

### Task 8: Split Oversized HTTP/SOCKS5/Pool/TLS Modules By Responsibility

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-tls/src/lib.rs`
- Create: `smelly-connect-cli/src/proxy/http/live.rs`
- Create: `smelly-connect-cli/src/proxy/http/body.rs`
- Create: `smelly-connect-cli/src/proxy/http/test_harness.rs`
- Create: `smelly-connect-cli/src/proxy/socks5/live.rs`
- Create: `smelly-connect-cli/src/proxy/socks5/test_harness.rs`
- Create: `smelly-connect-cli/src/pool/test_harness.rs`
- Create: `smelly-tls/src/handshake.rs`
- Create: `smelly-tls/src/parser.rs`
- Create: `smelly-tls/src/crypto.rs`

- [ ] **Step 1: Extract test harness sections first**

Do not change behavior. Move `#[cfg(any(test, debug_assertions))]` sections out before touching live logic.

- [ ] **Step 2: Extract body/header parsing helpers**

Keep live HTTP forwarding code in one file and parsing/body streaming in focused submodules.

- [ ] **Step 3: Split `smelly-tls` by protocol stage**

`parser.rs` for record/hello parsing, `handshake.rs` for transcript/builders, `crypto.rs` for PRF/RC4/HMAC helpers.

- [ ] **Step 4: Run focused tests after each extraction**

Run: `cargo test -p smelly-connect-cli http_proxy socks5_proxy pool -- --nocapture`
Run: `cargo test -p smelly-tls --quiet`
Expected: PASS after each extraction batch.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add smelly-connect-cli/src/proxy smelly-connect-cli/src/pool smelly-tls/src
git commit -m "refactor: split oversized proxy pool and tls modules"
```

### Task 9: Remove Dead Configuration And Nominal-Only Domain Wrappers

**Files:**
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect/src/domain/mod.rs`
- Delete or collapse: `smelly-connect/src/domain/session.rs`
- Delete or collapse: `smelly-connect/src/domain/resolver.rs`
- Delete or collapse: `smelly-connect/src/domain/stream.rs`
- Delete or collapse: `smelly-connect/src/domain/connect_plan.rs`
- Test: `smelly-connect/tests/public_api_audit.rs`
- Test: `smelly-connect/tests/ui/legacy_public_api.rs`

- [ ] **Step 1: Decide whether `selection` is real or dead**

If no second strategy is shipping now, remove the config field and fixtures. If multiple strategies are intended soon, implement them before keeping the field.

- [ ] **Step 2: Remove nominal-only aliases**

Inline or delete aliases that do not enforce domain invariants.

- [ ] **Step 3: Preserve external API intentionally**

If public API changes are required, update UI/public API tests in the same commit.

- [ ] **Step 4: Run API-focused tests**

Run: `cargo test -p smelly-connect public_api_audit`
Run: `cargo test -p smelly-connect --test ui legacy_public_api -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/config.rs smelly-connect/src/domain smelly-connect/tests/public_api_audit.rs smelly-connect/tests/ui/legacy_public_api.rs
git commit -m "refactor: remove dead config and nominal domain wrappers"
```

### Task 10: Final Verification

**Files:**
- Verify only

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace --quiet`
Expected: PASS

- [ ] **Step 2: Run full clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

- [ ] **Step 3: Review file lengths**

Run: `find smelly-connect smelly-connect-cli smelly-tls -name '*.rs' -print0 | xargs -0 wc -l | sort -nr | sed -n '1,20p'`
Expected: no live production file remains in the previous 1800-2400 line range.

- [ ] **Step 4: Commit final verification checkpoint**

```bash
git commit --allow-empty -m "chore: verify code quality remediation"
```
