# EasyConnect Rust Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Linux-first Rust EasyConnect client library and daemon that supports password login, external captcha callback, client IP retrieval, session-backed TCP streams, and a local HTTP/HTTPS CONNECT proxy.

**Architecture:** Implement the EasyConnect control plane as async Rust modules, then layer an internal packet device plus user-space TCP/IP stack on top of the VPN send/receive channels to expose `AsyncRead + AsyncWrite` streams. Route all embedded connections, HTTP proxy traffic, and Reqwest/Hyper integration through one shared `EasyConnectSession`.

**Tech Stack:** Rust stable, Tokio, Reqwest, Hyper, rustls or native-tls for normal HTTPS control requests, a user-space TCP/IP stack crate selected during implementation, XML/text parsing helpers, tracing, cargo test.

---

## Planned File Structure

### Top-Level

- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `.gitignore`
- Create: `README.md`

### Library Modules

- Create: `src/config.rs`
- Create: `src/error.rs`
- Create: `src/session.rs`
- Create: `src/target.rs`

### Auth And Control Plane

- Create: `src/auth/mod.rs`
- Create: `src/auth/login.rs`
- Create: `src/auth/captcha.rs`
- Create: `src/protocol/mod.rs`
- Create: `src/protocol/control.rs`
- Create: `src/protocol/tls.rs`
- Create: `src/protocol/packet.rs`

### Resources And Resolution

- Create: `src/resource/mod.rs`
- Create: `src/resource/model.rs`
- Create: `src/resource/parse.rs`
- Create: `src/resolver/mod.rs`

### Transport

- Create: `src/transport/mod.rs`
- Create: `src/transport/device.rs`
- Create: `src/transport/stack.rs`
- Create: `src/transport/stream.rs`

### Integration Surfaces

- Create: `src/proxy/mod.rs`
- Create: `src/proxy/http.rs`
- Create: `src/integration/mod.rs`
- Create: `src/integration/reqwest.rs`

### Tests

- Create: `tests/auth_login.rs`
- Create: `tests/resource_parse.rs`
- Create: `tests/routing.rs`
- Create: `tests/resolver.rs`
- Create: `tests/transport_stack.rs`
- Create: `tests/http_proxy.rs`
- Create: `tests/reqwest_integration.rs`
- Create: `tests/fixtures/login_auth_requires_captcha.xml`
- Create: `tests/fixtures/login_psw_success.xml`
- Create: `tests/fixtures/resource_sample.xml`
- Create: `tests/fixtures/conf_sample.xml`
- Create: `tests/fixtures/request_ip_reply.bin`

## Task 1: Scaffold The Cargo Project And Public API Skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/config.rs`
- Create: `src/error.rs`
- Create: `src/session.rs`
- Create: `src/target.rs`
- Create: `.gitignore`
- Create: `README.md`
- Test: `tests/auth_login.rs`

- [ ] **Step 1: Write the failing smoke test for public types**

```rust
use smelly_connect::{EasyConnectConfig, TargetAddr};

#[test]
fn public_api_smoke_compiles() {
    let _cfg = EasyConnectConfig::new("rvpn.example.com", "user", "pass");
    let _target = TargetAddr::from(("libdb.zju.edu.cn", 443));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test auth_login public_api_smoke_compiles -- --exact`
Expected: FAIL with missing crate items or missing Cargo manifest content

- [ ] **Step 3: Write the minimal project skeleton**

```rust
pub struct EasyConnectConfig {
    pub server: String,
    pub username: String,
    pub password: String,
}

impl EasyConnectConfig {
    pub fn new(server: impl Into<String>, username: impl Into<String>, password: impl Into<String>) -> Self {
        Self { server: server.into(), username: username.into(), password: password.into() }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test auth_login public_api_smoke_compiles -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml .gitignore README.md src/lib.rs src/main.rs src/config.rs src/error.rs src/session.rs src/target.rs tests/auth_login.rs
git commit -m "chore: scaffold Rust EasyConnect project"
```

## Task 2: Implement Login-Auth Parsing And Captcha Callback Contract

**Files:**
- Modify: `src/config.rs`
- Modify: `src/error.rs`
- Create: `src/auth/mod.rs`
- Create: `src/auth/login.rs`
- Create: `src/auth/captcha.rs`
- Test: `tests/auth_login.rs`
- Test Data: `tests/fixtures/login_auth_requires_captcha.xml`

- [ ] **Step 1: Write failing parser and callback tests**

```rust
#[tokio::test]
async fn login_auth_parses_captcha_requirement() {
    let body = include_str!("fixtures/login_auth_requires_captcha.xml");
    let parsed = smelly_connect::auth::parse_login_auth(body).unwrap();
    assert!(parsed.requires_captcha);
    assert_eq!(parsed.twfid, "dummy-twfid");
}

#[tokio::test]
async fn captcha_callback_receives_image_bytes() {
    let handler = smelly_connect::CaptchaHandler::from_async(|bytes, mime| async move {
        assert!(!bytes.is_empty());
        assert_eq!(mime.as_deref(), Some("image/jpeg"));
        Ok::<_, smelly_connect::CaptchaError>("1234".to_string())
    });
    let _ = handler.solve(vec![1, 2, 3], Some("image/jpeg".to_string())).await.unwrap();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test auth_login login_auth_parses_captcha_requirement -- --exact`
Expected: FAIL with missing auth parser or captcha handler types

- [ ] **Step 3: Implement parsing structs and callback wrapper**

```rust
pub struct LoginAuthResponse {
    pub twfid: String,
    pub rsa_key_hex: String,
    pub rsa_exp: u32,
    pub csrf_rand_code: Option<String>,
    pub requires_captcha: bool,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test auth_login`
Expected: PASS for parser and callback tests

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/error.rs src/auth/mod.rs src/auth/login.rs src/auth/captcha.rs tests/auth_login.rs tests/fixtures/login_auth_requires_captcha.xml
git commit -m "feat: add login auth parsing and captcha callback contract"
```

## Task 3: Implement Password Login Submission, Token Derivation, And Client-IP Parsing

**Files:**
- Modify: `src/error.rs`
- Modify: `src/auth/login.rs`
- Create: `src/protocol/mod.rs`
- Create: `src/protocol/control.rs`
- Create: `src/protocol/tls.rs`
- Test: `tests/auth_login.rs`
- Test Data: `tests/fixtures/login_psw_success.xml`
- Test Data: `tests/fixtures/request_ip_reply.bin`

- [ ] **Step 1: Write failing tests for password encryption, token derivation shape, and IP parsing**

```rust
#[test]
fn login_password_payload_uses_rsa_and_optional_csrf_suffix() {
    let encrypted = smelly_connect::auth::encrypt_password("pass", Some("csrf"), "ABCD", 65537).unwrap();
    assert!(!encrypted.is_empty());
}

#[test]
fn assigned_ip_reply_extracts_ipv4() {
    let reply = include_bytes!("fixtures/request_ip_reply.bin");
    let ip = smelly_connect::protocol::parse_assigned_ip_reply(reply).unwrap();
    assert_eq!(ip.to_string(), "10.0.0.8");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test auth_login`
Expected: FAIL with missing protocol helpers or parsing logic

- [ ] **Step 3: Implement the control-plane helpers**

```rust
pub fn parse_login_psw_success(body: &str) -> Result<String, AuthError> {
    // return updated TwfID on success
}

pub fn parse_assigned_ip_reply(reply: &[u8]) -> Result<std::net::Ipv4Addr, ProtocolError> {
    // verify type byte and read bytes 4..8
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test auth_login`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/error.rs src/auth/login.rs src/protocol/mod.rs src/protocol/control.rs src/protocol/tls.rs tests/auth_login.rs tests/fixtures/login_psw_success.xml tests/fixtures/request_ip_reply.bin
git commit -m "feat: add EasyConnect control-plane protocol helpers"
```

## Task 4: Implement Resource Parsing And Session Routing Rules

**Files:**
- Create: `src/resource/mod.rs`
- Create: `src/resource/model.rs`
- Create: `src/resource/parse.rs`
- Create: `src/resolver/mod.rs`
- Modify: `src/session.rs`
- Modify: `src/target.rs`
- Test: `tests/resource_parse.rs`
- Test: `tests/routing.rs`
- Test: `tests/resolver.rs`
- Test Data: `tests/fixtures/resource_sample.xml`
- Test Data: `tests/fixtures/conf_sample.xml`

- [ ] **Step 1: Write failing tests for resource parsing and route decisions**

```rust
#[test]
fn parses_domain_and_ip_resources() {
    let body = include_str!("fixtures/resource_sample.xml");
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.domain_rules.contains_key("zju.edu.cn"));
    assert!(!parsed.ip_rules.is_empty());
}

#[tokio::test]
async fn routing_rejects_non_resource_targets_by_default() {
    let session = smelly_connect::session::tests::fake_session_without_match();
    let err = session.plan_tcp_connect(("example.com", 443)).await.unwrap_err();
    assert!(matches!(err, smelly_connect::Error::Route(_)));
}

#[tokio::test]
async fn resolver_falls_back_from_remote_dns_to_system_dns() {
    let resolver = smelly_connect::resolver::tests::resolver_with_failing_remote();
    let ip = resolver.resolve_for_vpn("libdb.zju.edu.cn").await.unwrap();
    assert_eq!(ip.to_string(), "10.0.0.8");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test resource_parse --test routing --test resolver`
Expected: FAIL with missing resource parsers or routing planner

- [ ] **Step 3: Implement resource models, resolver pipeline, and route planner**

```rust
pub enum RoutePlan {
    VpnResolved(std::net::SocketAddr),
    Rejected,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test resource_parse --test routing --test resolver`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/resource/mod.rs src/resource/model.rs src/resource/parse.rs src/resolver/mod.rs src/session.rs src/target.rs tests/resource_parse.rs tests/routing.rs tests/resolver.rs tests/fixtures/resource_sample.xml tests/fixtures/conf_sample.xml
git commit -m "feat: add EasyConnect resources and routing policy"
```

## Task 5: Integrate Internal Packet Device And User-Space TCP/IP Stack

**Files:**
- Create: `src/protocol/packet.rs`
- Create: `src/transport/mod.rs`
- Create: `src/transport/device.rs`
- Create: `src/transport/stack.rs`
- Create: `src/transport/stream.rs`
- Modify: `Cargo.toml`
- Modify: `src/session.rs`
- Test: `tests/transport_stack.rs`

- [ ] **Step 1: Write failing transport tests around packet injection and TCP stream creation**

```rust
#[tokio::test]
async fn packet_device_forwards_frames_between_channels_and_stack() {
    let harness = smelly_connect::transport::tests::packet_harness();
    harness.inject_from_vpn(vec![0, 1, 2, 3]).await;
    assert_eq!(harness.read_for_stack().await, vec![0, 1, 2, 3]);
}

#[tokio::test]
async fn stack_can_create_outbound_tcp_stream_handle() {
    let harness = smelly_connect::transport::tests::stack_harness();
    let _stream = harness.connect(("10.0.0.8", 443)).await.unwrap();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test transport_stack`
Expected: FAIL with missing transport stack integration or harness helpers

- [ ] **Step 3: Choose and integrate one Rust user-space TCP/IP stack**

```rust
pub struct PacketDevice {
    rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
}

pub struct TransportStack {
    // wraps chosen async-friendly user-space stack
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test transport_stack`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/protocol/packet.rs src/transport/mod.rs src/transport/device.rs src/transport/stack.rs src/transport/stream.rs src/session.rs tests/transport_stack.rs
git commit -m "feat: add packet transport and user-space TCP stack"
```

## Task 6: Build Session Assembly And Connect API

**Files:**
- Modify: `src/config.rs`
- Modify: `src/error.rs`
- Modify: `src/session.rs`
- Modify: `src/lib.rs`
- Test: `tests/routing.rs`
- Test: `tests/transport_stack.rs`

- [ ] **Step 1: Write failing end-to-end session assembly tests with fakes**

```rust
#[tokio::test]
async fn config_connect_builds_session_with_client_ip() {
    let harness = smelly_connect::session::tests::login_harness();
    let session = harness.config().connect().await.unwrap();
    assert_eq!(session.client_ip().to_string(), "10.0.0.8");
}

#[tokio::test]
async fn session_connect_tcp_returns_async_stream() {
    let harness = smelly_connect::session::tests::stack_harness();
    let session = harness.ready_session().await;
    let _stream = session.connect_tcp(("10.0.0.8", 443)).await.unwrap();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test routing --test transport_stack`
Expected: FAIL with missing session assembly logic

- [ ] **Step 3: Implement `EasyConnectConfig::connect` and `EasyConnectSession::connect_tcp`**

```rust
impl EasyConnectConfig {
    pub async fn connect(self) -> Result<EasyConnectSession, Error> {
        // login, derive token, fetch IP, fetch resources, build resolver and transport
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test routing --test transport_stack`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/error.rs src/session.rs src/lib.rs tests/routing.rs tests/transport_stack.rs
git commit -m "feat: assemble EasyConnect sessions and connect API"
```

## Task 7: Add Local HTTP/HTTPS CONNECT Proxy

**Files:**
- Create: `src/proxy/mod.rs`
- Create: `src/proxy/http.rs`
- Modify: `src/session.rs`
- Modify: `src/main.rs`
- Test: `tests/http_proxy.rs`

- [ ] **Step 1: Write failing HTTP proxy tests for plain HTTP and CONNECT**

```rust
#[tokio::test]
async fn proxy_forwards_http_requests_through_session() {
    let harness = smelly_connect::proxy::tests::http_proxy_harness().await;
    let body = harness.get_via_proxy("http://intranet.zju.edu.cn/health").await;
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn proxy_supports_https_connect() {
    let harness = smelly_connect::proxy::tests::http_proxy_harness().await;
    harness.connect_tunnel("libdb.zju.edu.cn:443").await.unwrap();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test http_proxy`
Expected: FAIL with missing proxy server implementation

- [ ] **Step 3: Implement the session-backed HTTP proxy**

```rust
pub struct HttpProxyHandle {
    pub local_addr: std::net::SocketAddr,
}

impl EasyConnectSession {
    pub async fn start_http_proxy(&self, bind: std::net::SocketAddr) -> Result<HttpProxyHandle, Error> {
        // spawn server and tunnel through connect_tcp
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test http_proxy`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/proxy/mod.rs src/proxy/http.rs src/session.rs src/main.rs tests/http_proxy.rs
git commit -m "feat: add local EasyConnect HTTP proxy"
```

## Task 8: Add Reqwest/Hyper Integration And CLI Wiring

**Files:**
- Create: `src/integration/mod.rs`
- Create: `src/integration/reqwest.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Modify: `README.md`
- Test: `tests/reqwest_integration.rs`

- [ ] **Step 1: Write failing integration tests for direct client usage**

```rust
#[tokio::test]
async fn reqwest_helper_builds_client_over_session_connector() {
    let harness = smelly_connect::integration::tests::reqwest_harness().await;
    let client = harness.session.reqwest_client().unwrap();
    let body = harness.get_with(client, "https://libdb.zju.edu.cn/data").await;
    assert_eq!(body, "ok");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test reqwest_integration reqwest_helper_builds_client_over_session_connector -- --exact`
Expected: FAIL with missing Reqwest or Hyper integration helpers

- [ ] **Step 3: Implement integration helpers and CLI startup path**

```rust
impl EasyConnectSession {
    pub fn reqwest_client(&self) -> Result<reqwest::Client, Error> {
        // build client over session-backed connector
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test reqwest_integration reqwest_helper_builds_client_over_session_connector -- --exact`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/integration/mod.rs src/integration/reqwest.rs src/lib.rs src/main.rs README.md tests/reqwest_integration.rs
git commit -m "feat: add reqwest integration and CLI startup"
```

## Task 9: Run Full Verification And Document Manual Checks

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Write the manual verification checklist into the README**

```markdown
## Manual Verification

- password-only login
- captcha callback login
- client IP retrieval
- HTTP proxy request
- HTTPS CONNECT request
```

- [ ] **Step 2: Run the full test suite**

Run: `cargo test`
Expected: PASS

- [ ] **Step 3: Run formatting and linting**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS

- [ ] **Step 4: Run one real-environment smoke command**

Run: `cargo run -- --server <server> --username <user> --password <pass> --http-bind 127.0.0.1:18081`
Expected: login succeeds, proxy starts, and logs client IP

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: add verification checklist for EasyConnect rewrite"
```

## Plan Review Checklist

Review this plan against:

- `docs/specs/2026-03-25-easyconnect-rust-rewrite-design.md`
- no scope beyond EasyConnect v1
- no public UDP/TUN features
- all implementation work uses TDD
- every task ends with a git commit
- no files under `docs/superpowers/`

## Reviewer Fallback

The normal skill flow asks for a dedicated plan-review subagent. If subagent delegation is not available for this run, perform a local review using `writing-plans/plan-document-reviewer-prompt.md` before execution and record any fixes in git before coding.
