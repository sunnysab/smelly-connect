# Standard Proxy Implementations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hand-rolled SOCKS5 and HTTP proxy protocol handling in `smelly-connect-cli` with `fast-socks5` and `hyper` while preserving session-pool routing, timeout handling, runtime stats, and current operational logging.

**Architecture:** Keep listener lifecycle, pool selection, upstream connection, timeout mapping, stats, and logging in `smelly-connect-cli`. Delegate SOCKS5 handshake / command parsing / reply framing to `fast-socks5`, and delegate HTTP/1.1 request parsing / response generation / CONNECT upgrade handling to `hyper`.

**Tech Stack:** Rust, Tokio, `fast-socks5`, `hyper`, `hyper-util`, `http-body-util`

---

### Task 1: Establish Replacement Baseline

**Files:**
- Modify: `smelly-connect-cli/Cargo.toml`
- Test: existing `smelly-connect-cli` proxy tests

- [ ] **Step 1: Run targeted existing proxy tests as the pre-change baseline**

Run:
`cargo test -q -p smelly-connect-cli socks5_proxy_supports_tcp_connect -- --exact`
`cargo test -q -p smelly-connect-cli http_proxy_uses_pool_and_forwards_requests -- --exact`
`cargo test -q -p smelly-connect-cli http_connect_proxy_tunnels_bytes_through_selected_session -- --exact`

- [ ] **Step 2: Add dependency declarations for standard protocol implementations**

Add `fast-socks5`, `hyper`, `hyper-util`, `http-body-util`, `http`, and `bytes` to `smelly-connect-cli/Cargo.toml`.

- [ ] **Step 3: Run `cargo check` to verify dependency graph compiles**

Run: `cargo check -p smelly-connect-cli`

### Task 2: Replace SOCKS5 Protocol Handling With `fast-socks5`

**Files:**
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Test: existing SOCKS5 tests in `smelly-connect-cli`

- [ ] **Step 1: Add or adjust a SOCKS5 regression test for the desired protocol behavior**

Use the current SOCKS5 test harness so the test exercises the public listener, not helper internals.

- [ ] **Step 2: Run the targeted SOCKS5 test to verify the old implementation does not satisfy the new expectation**

Run the exact test only and confirm failure is attributable to the protocol implementation gap.

- [ ] **Step 3: Replace manual handshake parsing with `fast-socks5` explicit server protocol flow**

Use `accept_no_auth` / `read_command` / reply helpers while preserving:
`SessionPool` selection, connect timeout mapping, runtime stats, and structured logging.

- [ ] **Step 4: Run targeted SOCKS5 tests**

Run:
`cargo test -q -p smelly-connect-cli socks5_proxy_supports_tcp_connect -- --exact`
`cargo test -q -p smelly-connect-cli socks5_proxy_returns_failure_when_no_ready_session_exists -- --exact`
`cargo test -q -p smelly-connect-cli socks5_proxy_updates_runtime_stats_after_tunneling -- --exact`
`cargo test -q -p smelly-connect-cli socks5_proxy_rejects_unsupported_auth_methods -- --exact`
`cargo test -q -p smelly-connect-cli socks5_proxy_rejects_unsupported_commands_with_reply -- --exact`

### Task 3: Replace HTTP Protocol Handling With `hyper`

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Test: existing HTTP proxy tests in `smelly-connect-cli`

- [ ] **Step 1: Add or adjust an HTTP regression test around parser / request handling expectations**

Prefer a test that validates behavior through the listener instead of asserting on internal parser functions.

- [ ] **Step 2: Run the targeted HTTP test to verify the pre-change implementation fails the new expectation**

Keep the failure scoped and explicit.

- [ ] **Step 3: Replace manual request parsing with `hyper` HTTP/1.1 server handling**

Use `hyper` for request decoding and response encoding, including CONNECT upgrade handling, while preserving:
pool selection, connect timeout mapping, current status-code behavior, logging, and runtime stats.

- [ ] **Step 4: Run targeted HTTP tests**

Run:
`cargo test -q -p smelly-connect-cli http_proxy_uses_pool_and_forwards_requests -- --exact`
`cargo test -q -p smelly-connect-cli http_connect_proxy_tunnels_bytes_through_selected_session -- --exact`
`cargo test -q -p smelly-connect-cli http_proxy_updates_runtime_stats_after_forwarding -- --exact`
`cargo test -q -p smelly-connect-cli http_proxy_streams_split_request_body_to_upstream -- --exact`
`cargo test -q -p smelly-connect-cli http_proxy_streams_split_chunked_request_body_to_upstream -- --exact`
`cargo test -q -p smelly-connect-cli http_proxy_handles_expect_100_continue_requests -- --exact`

### Task 4: Full Verification

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/Cargo.toml`

- [ ] **Step 1: Run full proxy-related test slice**

Run:
`cargo test -q -p smelly-connect-cli socks5_proxy`
`cargo test -q -p smelly-connect-cli http_proxy`
`cargo test -q -p smelly-connect-cli http_connect`

- [ ] **Step 2: Run package-level verification**

Run:
`cargo test -q -p smelly-connect-cli`
`cargo check -q -p smelly-connect`

- [ ] **Step 3: Review diff for accidental behavior or scope creep**

Run: `git diff -- smelly-connect-cli/Cargo.toml smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/src/proxy/socks5.rs`
