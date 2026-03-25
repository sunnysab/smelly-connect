# smelly-connect v0.2 Architecture Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure `smelly-connect` into a protocol-kernel-centered library with stable facade APIs, explicit runtime ownership, and `0.2.0` package metadata after the redesign is complete.

**Architecture:** Preserve the current behavior as a `v0.1` baseline, then peel pure protocol logic into `kernel`, move orchestration and task ownership into `runtime`, define stable library concepts in `domain`, and reassemble the public surface through a small `facade`. Finish by rewiring integrations to consume `Session` capabilities only, removing leaked internals from the public API.

**Tech Stack:** Rust stable, Cargo workspace, Tokio async runtime, Reqwest, OpenSSL, smoltcp, `smelly-tls`, cargo test, git tags and commits.

---

## Planned File Structure

### Create

- `docs/superpowers/plans/2026-03-25-smelly-connect-v0-2-architecture-plan.md`
- `smelly-connect/src/facade/mod.rs`
- `smelly-connect/src/facade/client.rs`
- `smelly-connect/src/facade/session.rs`
- `smelly-connect/src/domain/mod.rs`
- `smelly-connect/src/domain/connect_target.rs`
- `smelly-connect/src/domain/connect_plan.rs`
- `smelly-connect/src/domain/keepalive.rs`
- `smelly-connect/src/domain/resolver.rs`
- `smelly-connect/src/domain/route_policy.rs`
- `smelly-connect/src/domain/session.rs`
- `smelly-connect/src/domain/session_info.rs`
- `smelly-connect/src/domain/stream.rs`
- `smelly-connect/src/kernel/mod.rs`
- `smelly-connect/src/kernel/control/mod.rs`
- `smelly-connect/src/kernel/control/messages.rs`
- `smelly-connect/src/kernel/control/parser.rs`
- `smelly-connect/src/kernel/control/encoder.rs`
- `smelly-connect/src/kernel/tunnel/mod.rs`
- `smelly-connect/src/kernel/tunnel/token.rs`
- `smelly-connect/src/kernel/tunnel/handshake.rs`
- `smelly-connect/src/kernel/tunnel/parser.rs`
- `smelly-connect/src/kernel/tunnel/cipher.rs`
- `smelly-connect/src/runtime/mod.rs`
- `smelly-connect/src/runtime/control_plane/mod.rs`
- `smelly-connect/src/runtime/control_plane/client.rs`
- `smelly-connect/src/runtime/control_plane/flow.rs`
- `smelly-connect/src/runtime/control_plane/types.rs`
- `smelly-connect/src/runtime/data_plane/mod.rs`
- `smelly-connect/src/runtime/data_plane/tunnel_factory.rs`
- `smelly-connect/src/runtime/data_plane/packet_pump.rs`
- `smelly-connect/src/runtime/data_plane/netstack.rs`
- `smelly-connect/src/runtime/data_plane/transport.rs`
- `smelly-connect/src/runtime/tasks/mod.rs`
- `smelly-connect/src/runtime/tasks/keepalive.rs`
- `smelly-connect/src/runtime/tasks/supervisor.rs`
- `smelly-connect/src/test_support/mod.rs`
- `smelly-connect/tests/kernel_control.rs`
- `smelly-connect/tests/kernel_tunnel.rs`
- `smelly-connect/tests/facade_session.rs`
- `smelly-connect/tests/lifecycle_handles.rs`
- `smelly-connect/tests/public_api_audit.rs`
- `smelly-connect/tests/ui/legacy_public_api.rs`

### Modify

- `smelly-connect/Cargo.toml`
- `smelly-connect/src/lib.rs`
- `smelly-connect/src/main.rs`
- `smelly-connect/src/error.rs`
- `smelly-connect/src/config.rs`
- `smelly-connect/src/session.rs`
- `smelly-connect/src/target.rs`
- `smelly-connect/src/auth/mod.rs`
- `smelly-connect/src/auth/login.rs`
- `smelly-connect/src/auth/control.rs`
- `smelly-connect/src/resource/mod.rs`
- `smelly-connect/src/resource/model.rs`
- `smelly-connect/src/resource/parse.rs`
- `smelly-connect/src/resolver/mod.rs`
- `smelly-connect/src/transport/mod.rs`
- `smelly-connect/src/transport/device.rs`
- `smelly-connect/src/transport/netstack.rs`
- `smelly-connect/src/transport/stack.rs`
- `smelly-connect/src/transport/stream.rs`
- `smelly-connect/src/protocol/mod.rs`
- `smelly-connect/src/protocol/control.rs`
- `smelly-connect/src/protocol/tls.rs`
- `smelly-connect/src/integration/mod.rs`
- `smelly-connect/src/integration/http_proxy.rs`
- `smelly-connect/src/integration/reqwest.rs`
- `smelly-connect/src/proxy/mod.rs`
- `smelly-connect/src/proxy/http.rs`
- `README.md`
- `smelly-connect/README.md`

### Delete Or Reduce To Re-exports

- `smelly-connect/src/config.rs`
- `smelly-connect/src/session.rs`
- `smelly-connect/src/target.rs`
- `smelly-connect/src/auth/control.rs`
- `smelly-connect/src/protocol/control.rs`
- `smelly-connect/src/protocol/tls.rs`
- `smelly-connect/src/transport/stack.rs`

These files may either be deleted after migration or reduced to deprecated compatibility re-exports during the transition. Do not keep duplicated logic in both old and new locations.

### Existing Tests To Keep Green

- `smelly-connect/tests/auth_login.rs`
- `smelly-connect/tests/connect_control_plane.rs`
- `smelly-connect/tests/http_proxy.rs`
- `smelly-connect/tests/protocol_tls.rs`
- `smelly-connect/tests/reqwest_integration.rs`
- `smelly-connect/tests/resolver.rs`
- `smelly-connect/tests/resource_parse.rs`
- `smelly-connect/tests/routing.rs`
- `smelly-connect/tests/transport_stack.rs`

## Task 1: Freeze The v0.1 Baseline And Scaffold The New Layer Roots

**Files:**
- Modify: `smelly-connect/Cargo.toml`
- Modify: `smelly-connect/src/lib.rs`
- Create: `smelly-connect/src/facade/mod.rs`
- Create: `smelly-connect/src/domain/mod.rs`
- Create: `smelly-connect/src/domain/session.rs`
- Create: `smelly-connect/src/kernel/mod.rs`
- Create: `smelly-connect/src/runtime/mod.rs`
- Test: `smelly-connect/tests/facade_session.rs`

- [ ] **Step 1: Verify the `v0.1` tag does not already exist**

Run: `git rev-parse -q --verify refs/tags/v0.1`
Expected: command exits non-zero because the tag is not present yet

- [ ] **Step 2: Create the baseline tag before any code edits**

Run: `git tag v0.1`
Expected: command exits successfully and `git tag --list v0.1` prints `v0.1`

- [ ] **Step 3: Write the failing public-surface smoke test**

```rust
use smelly_connect::{EasyConnectClient, Session};

#[test]
fn facade_types_are_exported() {
    let _ = std::any::TypeId::of::<EasyConnectClient>();
    let _ = std::any::TypeId::of::<Session>();
}
```

- [ ] **Step 4: Run the smoke test to verify it fails**

Run: `cargo test -p smelly-connect --test facade_session facade_types_are_exported -- --exact`
Expected: FAIL with unresolved imports for `EasyConnectClient` and `Session`

- [ ] **Step 5: Add the new root modules and temporary re-exports**

```rust
// smelly-connect/src/lib.rs
pub mod facade;
pub mod domain;
pub mod kernel;
pub mod runtime;

pub use facade::client::EasyConnectClient;
pub use domain::session::Session;

// smelly-connect/src/domain/session.rs
pub struct Session;
```

- [ ] **Step 6: Run the smoke test to verify the new layer roots compile**

Run: `cargo test -p smelly-connect --test facade_session facade_types_are_exported -- --exact`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add smelly-connect/Cargo.toml smelly-connect/src/lib.rs smelly-connect/src/facade/mod.rs smelly-connect/src/domain/mod.rs smelly-connect/src/domain/session.rs smelly-connect/src/kernel/mod.rs smelly-connect/src/runtime/mod.rs smelly-connect/tests/facade_session.rs
git commit -m "refactor: scaffold v0.2 architecture layers"
```

## Task 2: Extract Pure Control-Plane Protocol Into `kernel::control`

**Files:**
- Create: `smelly-connect/src/kernel/control/mod.rs`
- Create: `smelly-connect/src/kernel/control/messages.rs`
- Create: `smelly-connect/src/kernel/control/parser.rs`
- Create: `smelly-connect/src/kernel/control/encoder.rs`
- Modify: `smelly-connect/src/auth/login.rs`
- Modify: `smelly-connect/src/resource/parse.rs`
- Modify: `smelly-connect/src/protocol/control.rs`
- Modify: `smelly-connect/src/protocol/mod.rs`
- Test: `smelly-connect/tests/kernel_control.rs`
- Test: `smelly-connect/tests/auth_login.rs`
- Test: `smelly-connect/tests/resource_parse.rs`

- [ ] **Step 1: Write the failing kernel control tests**

```rust
use smelly_connect::kernel::control::{
    parse_login_auth_challenge,
    parse_login_success,
    parse_resource_document,
};

#[test]
fn login_auth_challenge_extracts_required_fields() {
    let body = include_str!("fixtures/login_auth_requires_captcha.xml");
    let parsed = parse_login_auth_challenge(body).unwrap();
    assert_eq!(parsed.twfid, "dummy-twfid");
    assert!(parsed.requires_captcha);
}
```

- [ ] **Step 2: Run the new and existing parser tests to verify failure**

Run: `cargo test -p smelly-connect --test kernel_control --test auth_login --test resource_parse`
Expected: FAIL with missing `kernel::control` functions or types

- [ ] **Step 3: Move pure parsing and message types into `kernel::control`**

```rust
pub struct LoginAuthChallenge {
    pub twfid: String,
    pub rsa_key_hex: String,
    pub rsa_exp: u32,
    pub csrf_rand_code: Option<String>,
    pub legacy_cipher_hint: Option<String>,
    pub requires_captcha: bool,
}

pub fn parse_login_auth_challenge(body: &str) -> Result<LoginAuthChallenge, ControlParseError> { /* ... */ }
pub fn parse_login_success(body: &str, current_twfid: &str) -> Result<String, ControlParseError> { /* ... */ }
pub fn parse_resource_document(body: &str) -> Result<ResourceDocument, ControlParseError> { /* ... */ }
```

- [ ] **Step 4: Rewire old call sites to delegate to `kernel::control` without behavior changes**

Run: `cargo test -p smelly-connect --test kernel_control --test auth_login --test resource_parse`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/kernel/control/mod.rs smelly-connect/src/kernel/control/messages.rs smelly-connect/src/kernel/control/parser.rs smelly-connect/src/kernel/control/encoder.rs smelly-connect/src/auth/login.rs smelly-connect/src/resource/parse.rs smelly-connect/src/protocol/control.rs smelly-connect/src/protocol/mod.rs smelly-connect/tests/kernel_control.rs smelly-connect/tests/auth_login.rs smelly-connect/tests/resource_parse.rs
git commit -m "refactor: extract control-plane kernel"
```

## Task 3: Extract Pure Tunnel Protocol Into `kernel::tunnel`

**Files:**
- Create: `smelly-connect/src/kernel/tunnel/mod.rs`
- Create: `smelly-connect/src/kernel/tunnel/token.rs`
- Create: `smelly-connect/src/kernel/tunnel/handshake.rs`
- Create: `smelly-connect/src/kernel/tunnel/parser.rs`
- Create: `smelly-connect/src/kernel/tunnel/cipher.rs`
- Modify: `smelly-connect/src/protocol/tls.rs`
- Modify: `smelly-connect/src/auth/control.rs`
- Modify: `smelly-connect/src/protocol/mod.rs`
- Test: `smelly-connect/tests/kernel_tunnel.rs`
- Test: `smelly-connect/tests/protocol_tls.rs`

- [ ] **Step 1: Write the failing tunnel-kernel tests**

```rust
use std::net::Ipv4Addr;

use smelly_connect::kernel::tunnel::{
    build_recv_handshake,
    build_request_ip_message,
    build_send_handshake,
    derive_token,
};

#[test]
fn request_ip_message_layout_matches_existing_behavior() {
    let token = derive_token("0123456789abcdef0123456789abcde", "twfid").unwrap();
    let message = build_request_ip_message(&token);
    assert_eq!(message.len(), 64);
}

#[test]
fn stream_handshakes_encode_client_ip_octets() {
    let token = derive_token("0123456789abcdef0123456789abcde", "twfid").unwrap();
    let send = build_send_handshake(&token, Ipv4Addr::new(10, 0, 0, 8));
    let recv = build_recv_handshake(&token, Ipv4Addr::new(10, 0, 0, 8));
    assert_eq!(send[0], 0x05);
    assert_eq!(recv[0], 0x06);
}
```

- [ ] **Step 2: Run the protocol tests to verify failure**

Run: `cargo test -p smelly-connect --test kernel_tunnel --test protocol_tls`
Expected: FAIL with unresolved imports from `kernel::tunnel`

- [ ] **Step 3: Move token derivation, handshake builders, reply parsers, and cipher policy into `kernel::tunnel`**

```rust
pub struct DerivedToken([u8; 48]);

pub fn derive_token(server_session_id_hex: &str, twfid: &str) -> Result<DerivedToken, TunnelProtocolError> { /* ... */ }
pub fn build_request_ip_message(token: &DerivedToken) -> Vec<u8> { /* ... */ }
pub fn build_send_handshake(token: &DerivedToken, client_ip: Ipv4Addr) -> Vec<u8> { /* ... */ }
pub fn build_recv_handshake(token: &DerivedToken, client_ip: Ipv4Addr) -> Vec<u8> { /* ... */ }
pub fn choose_cipher_suite(hint: Option<&str>) -> CipherSuitePolicy { /* ... */ }
```

- [ ] **Step 4: Repoint runtime callers through the new kernel module and rerun tests**

Run: `cargo test -p smelly-connect --test kernel_tunnel --test protocol_tls`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/kernel/tunnel/mod.rs smelly-connect/src/kernel/tunnel/token.rs smelly-connect/src/kernel/tunnel/handshake.rs smelly-connect/src/kernel/tunnel/parser.rs smelly-connect/src/kernel/tunnel/cipher.rs smelly-connect/src/protocol/tls.rs smelly-connect/src/auth/control.rs smelly-connect/src/protocol/mod.rs smelly-connect/tests/kernel_tunnel.rs smelly-connect/tests/protocol_tls.rs
git commit -m "refactor: extract tunnel kernel"
```

## Task 4: Define Stable Domain Types And Replace Loose Parameter Bundles

**Files:**
- Create: `smelly-connect/src/domain/connect_target.rs`
- Create: `smelly-connect/src/domain/connect_plan.rs`
- Create: `smelly-connect/src/domain/keepalive.rs`
- Create: `smelly-connect/src/domain/resolver.rs`
- Create: `smelly-connect/src/domain/route_policy.rs`
- Create: `smelly-connect/src/domain/session.rs`
- Create: `smelly-connect/src/domain/session_info.rs`
- Create: `smelly-connect/src/domain/stream.rs`
- Modify: `smelly-connect/src/lib.rs`
- Modify: `smelly-connect/src/target.rs`
- Modify: `smelly-connect/src/resolver/mod.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/transport/stream.rs`
- Test: `smelly-connect/tests/routing.rs`
- Test: `smelly-connect/tests/resolver.rs`
- Test: `smelly-connect/tests/facade_session.rs`

- [ ] **Step 1: Write the failing domain-type tests**

```rust
use smelly_connect::{ConnectTarget, KeepalivePolicy, SessionInfo};

#[test]
fn connect_target_accepts_host_port_and_socket_addr() {
    let host = ConnectTarget::from(("jwxt.sit.edu.cn", 443));
    assert_eq!(host.port(), 443);
}

#[test]
fn session_info_exposes_client_ip() {
    let info = SessionInfo::new("10.0.0.8".parse().unwrap());
    assert_eq!(info.client_ip().to_string(), "10.0.0.8");
}
```

- [ ] **Step 2: Run the routing and facade tests to verify failure**

Run: `cargo test -p smelly-connect --test routing --test resolver --test facade_session`
Expected: FAIL with missing `ConnectTarget`, `KeepalivePolicy`, or `SessionInfo`

- [ ] **Step 3: Introduce the stable domain vocabulary and typed context objects**

```rust
pub type SessionFut<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

pub(crate) trait SessionRuntime: Send + Sync {
    fn connect(&self, target: ConnectTarget) -> SessionFut<'_, VpnStream>;
    fn start_http_proxy(&self, bind: SocketAddr) -> SessionFut<'_, ProxyHandle>;
    fn start_keepalive(&self, target: ConnectTarget) -> SessionFut<'_, KeepaliveHandle>;
}

pub struct SessionInfo {
    client_ip: Ipv4Addr,
}

pub enum KeepalivePolicy {
    Disabled,
    Icmp { target: ConnectTarget, interval: Duration },
}

pub struct Session {
    info: SessionInfo,
    pub(crate) inner: Arc<dyn SessionRuntime>,
}

pub struct ProxyHandle {
    local_addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

pub struct KeepaliveHandle {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}
```

- [ ] **Step 4: Rework old target/resolver/session helpers to use the new domain types and re-export them from the crate root**

Run: `cargo test -p smelly-connect --test routing --test resolver --test facade_session`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/domain/mod.rs smelly-connect/src/domain/connect_target.rs smelly-connect/src/domain/connect_plan.rs smelly-connect/src/domain/keepalive.rs smelly-connect/src/domain/resolver.rs smelly-connect/src/domain/route_policy.rs smelly-connect/src/domain/session.rs smelly-connect/src/domain/session_info.rs smelly-connect/src/domain/stream.rs smelly-connect/src/lib.rs smelly-connect/src/target.rs smelly-connect/src/resolver/mod.rs smelly-connect/src/session.rs smelly-connect/src/transport/stream.rs smelly-connect/tests/routing.rs smelly-connect/tests/resolver.rs smelly-connect/tests/facade_session.rs
git commit -m "refactor: define stable domain types"
```

## Task 5: Build `runtime::control_plane` And Typed Session Seeds

**Files:**
- Create: `smelly-connect/src/runtime/control_plane/mod.rs`
- Create: `smelly-connect/src/runtime/control_plane/client.rs`
- Create: `smelly-connect/src/runtime/control_plane/flow.rs`
- Create: `smelly-connect/src/runtime/control_plane/types.rs`
- Modify: `smelly-connect/src/auth/control.rs`
- Modify: `smelly-connect/src/auth/mod.rs`
- Modify: `smelly-connect/src/config.rs`
- Modify: `smelly-connect/src/error.rs`
- Test: `smelly-connect/tests/connect_control_plane.rs`
- Test: `smelly-connect/tests/auth_login.rs`

- [ ] **Step 1: Write the failing typed-seed and control-plane tests**

```rust
use smelly_connect::runtime::control_plane::AuthenticatedSessionSeed;

#[test]
fn authenticated_session_seed_carries_resources_and_tunnel_bootstrap() {
    let _ = std::any::TypeId::of::<AuthenticatedSessionSeed>();
}
```

- [ ] **Step 2: Run control-plane tests to verify failure**

Run: `cargo test -p smelly-connect --test connect_control_plane --test auth_login authenticated_session_seed_carries_resources_and_tunnel_bootstrap -- --exact`
Expected: FAIL with missing `AuthenticatedSessionSeed` or `runtime::control_plane`

- [ ] **Step 3: Split runtime I/O orchestration out of `auth::control`**

```rust
pub type ControlPlaneFut<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ControlPlaneError>> + Send + 'a>>;

pub struct TunnelBootstrap {
    pub server_addr: SocketAddr,
    pub client_ip: Ipv4Addr,
    pub token: DerivedToken,
    pub cipher_policy: CipherSuitePolicy,
}

pub struct AuthenticatedSessionSeed {
    pub session_cookie: String,
    pub resources: ResourceSet,
    pub tunnel_bootstrap: TunnelBootstrap,
}

pub trait ControlPlaneClient {
    fn fetch_login_auth(&self, base_url: &str) -> ControlPlaneFut<'_, LoginAuthChallenge>;
    fn fetch_captcha(&self, base_url: &str, twfid: &str) -> ControlPlaneFut<'_, CaptchaImage>;
    fn submit_login(&self, req: LoginRequest<'_>) -> ControlPlaneFut<'_, LoginSuccess>;
    fn fetch_resources(&self, base_url: &str, twfid: &str) -> ControlPlaneFut<'_, ResourceSet>;
}
```

- [ ] **Step 4: Keep the existing control-plane tests green through the new runtime flow**

Run: `cargo test -p smelly-connect --test connect_control_plane --test auth_login`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/runtime/control_plane/mod.rs smelly-connect/src/runtime/control_plane/client.rs smelly-connect/src/runtime/control_plane/flow.rs smelly-connect/src/runtime/control_plane/types.rs smelly-connect/src/auth/control.rs smelly-connect/src/auth/mod.rs smelly-connect/src/config.rs smelly-connect/src/error.rs smelly-connect/tests/connect_control_plane.rs smelly-connect/tests/auth_login.rs
git commit -m "refactor: move control-plane flow into runtime"
```

## Task 6: Build `runtime::data_plane` And Explicit Task Ownership

**Files:**
- Create: `smelly-connect/src/runtime/data_plane/mod.rs`
- Create: `smelly-connect/src/runtime/data_plane/tunnel_factory.rs`
- Create: `smelly-connect/src/runtime/data_plane/packet_pump.rs`
- Create: `smelly-connect/src/runtime/data_plane/netstack.rs`
- Create: `smelly-connect/src/runtime/data_plane/transport.rs`
- Create: `smelly-connect/src/runtime/tasks/mod.rs`
- Create: `smelly-connect/src/runtime/tasks/keepalive.rs`
- Create: `smelly-connect/src/runtime/tasks/supervisor.rs`
- Modify: `smelly-connect/src/transport/device.rs`
- Modify: `smelly-connect/src/transport/netstack.rs`
- Modify: `smelly-connect/src/transport/stack.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/error.rs`
- Test: `smelly-connect/tests/transport_stack.rs`
- Test: `smelly-connect/tests/lifecycle_handles.rs`

- [ ] **Step 1: Write the failing runtime transport and handle-lifecycle tests**

```rust
use smelly_connect::{KeepaliveHandle, ProxyHandle};

#[tokio::test]
async fn keepalive_handle_supports_explicit_shutdown() {
    let _ = std::any::TypeId::of::<KeepaliveHandle>();
}

#[tokio::test]
async fn proxy_handle_supports_explicit_shutdown() {
    let _ = std::any::TypeId::of::<ProxyHandle>();
}
```

- [ ] **Step 2: Run transport and lifecycle tests to verify failure**

Run: `cargo test -p smelly-connect --test transport_stack --test lifecycle_handles`
Expected: FAIL with missing handle types or missing shutdown methods

- [ ] **Step 3: Move packet pumping, netstack driving, and task supervision into runtime**

```rust
pub type TransportFut<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, TransportError>> + Send + 'a>>;

pub trait Transport: Send + Sync {
    fn connect(&self, target: ConnectTarget) -> TransportFut<'_, VpnStream>;
    fn icmp_ping(&self, target: Ipv4Addr) -> TransportFut<'_, ()>;
}

impl ProxyHandle {
    pub fn local_addr(&self) -> SocketAddr { self.local_addr }
    pub async fn shutdown(mut self) -> Result<(), Error> { /* ... */ }
}

impl KeepaliveHandle {
    pub async fn shutdown(mut self) -> Result<(), Error> { /* ... */ }
}
```

- [ ] **Step 4: Rewire the old `transport::*` surface to delegate or shrink to compatibility shims**

Run: `cargo test -p smelly-connect --test transport_stack --test lifecycle_handles`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/runtime/data_plane/mod.rs smelly-connect/src/runtime/data_plane/tunnel_factory.rs smelly-connect/src/runtime/data_plane/packet_pump.rs smelly-connect/src/runtime/data_plane/netstack.rs smelly-connect/src/runtime/data_plane/transport.rs smelly-connect/src/runtime/tasks/mod.rs smelly-connect/src/runtime/tasks/keepalive.rs smelly-connect/src/runtime/tasks/supervisor.rs smelly-connect/src/transport/device.rs smelly-connect/src/transport/netstack.rs smelly-connect/src/transport/stack.rs smelly-connect/src/session.rs smelly-connect/src/error.rs smelly-connect/tests/transport_stack.rs smelly-connect/tests/lifecycle_handles.rs
git commit -m "refactor: move data-plane runtime behind explicit handles"
```

## Task 7: Assemble The New Facade And Rework Integrations

**Files:**
- Create: `smelly-connect/src/facade/client.rs`
- Create: `smelly-connect/src/facade/session.rs`
- Modify: `smelly-connect/src/config.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/integration/mod.rs`
- Create: `smelly-connect/src/integration/http_proxy.rs`
- Modify: `smelly-connect/src/integration/reqwest.rs`
- Modify: `smelly-connect/src/proxy/mod.rs`
- Modify: `smelly-connect/src/proxy/http.rs`
- Modify: `smelly-connect/src/lib.rs`
- Test: `smelly-connect/tests/facade_session.rs`
- Test: `smelly-connect/tests/http_proxy.rs`
- Test: `smelly-connect/tests/reqwest_integration.rs`

- [ ] **Step 1: Write the failing facade and integration tests**

```rust
use smelly_connect::EasyConnectClient;

#[tokio::test]
async fn session_reqwest_client_does_not_require_mem_forget() {
    let _ = std::any::TypeId::of::<EasyConnectClient>();
}
```

- [ ] **Step 2: Run facade, proxy, and reqwest tests to verify failure**

Run: `cargo test -p smelly-connect --test facade_session --test http_proxy --test reqwest_integration`
Expected: FAIL with missing `EasyConnectClient::builder`, missing `Session::start_http_proxy`, or outdated integration assumptions

- [ ] **Step 3: Implement the public facade and move canonical HTTP proxy logic under `integration`**

```rust
pub struct EasyConnectClientBuilder { /* user-facing options only */ }
pub struct EasyConnectClient { /* runtime assembly entrypoint */ }

impl EasyConnectClient {
    pub fn builder(server: impl Into<String>) -> EasyConnectClientBuilder { /* ... */ }
    pub async fn connect(&self) -> Result<Session, Error> { /* ... */ }
}

impl Session {
    pub async fn start_http_proxy(&self, bind: SocketAddr) -> Result<ProxyHandle, Error> { /* delegates to integration::http_proxy */ }
    pub async fn reqwest_client(&self) -> Result<reqwest::Client, Error> { /* ... */ }
}
```

After this task, `src/proxy/*` should either be deleted or reduced to shallow compatibility shims that delegate into `integration::http_proxy`. Do not keep a second canonical proxy implementation there.

- [ ] **Step 4: Remove hidden resource leaks from integrations and rerun tests**

Run: `cargo test -p smelly-connect --test facade_session --test http_proxy --test reqwest_integration`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/facade/client.rs smelly-connect/src/facade/session.rs smelly-connect/src/config.rs smelly-connect/src/session.rs smelly-connect/src/integration/mod.rs smelly-connect/src/integration/http_proxy.rs smelly-connect/src/integration/reqwest.rs smelly-connect/src/proxy/mod.rs smelly-connect/src/proxy/http.rs smelly-connect/src/lib.rs smelly-connect/tests/facade_session.rs smelly-connect/tests/http_proxy.rs smelly-connect/tests/reqwest_integration.rs
git commit -m "refactor: expose v0.2 facade and integrations"
```

## Task 8: Remove Legacy Surface Area, Move Test Support, And Tighten Errors

**Files:**
- Create: `smelly-connect/src/test_support/mod.rs`
- Modify: `smelly-connect/Cargo.toml`
- Modify: `smelly-connect/src/error.rs`
- Modify: `smelly-connect/src/auth/control.rs`
- Modify: `smelly-connect/src/auth/mod.rs`
- Modify: `smelly-connect/src/protocol/mod.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/transport/mod.rs`
- Modify: `smelly-connect/src/integration/mod.rs`
- Modify: `smelly-connect/src/integration/http_proxy.rs`
- Modify: `smelly-connect/src/proxy/mod.rs`
- Modify: `smelly-connect/src/lib.rs`
- Test: `smelly-connect/tests/public_api_audit.rs`
- Test: `smelly-connect/tests/ui/legacy_public_api.rs`
- Modify: `smelly-connect/tests/auth_login.rs`
- Modify: `smelly-connect/tests/connect_control_plane.rs`
- Modify: `smelly-connect/tests/http_proxy.rs`
- Modify: `smelly-connect/tests/protocol_tls.rs`
- Modify: `smelly-connect/tests/reqwest_integration.rs`
- Modify: `smelly-connect/tests/resolver.rs`
- Modify: `smelly-connect/tests/resource_parse.rs`
- Modify: `smelly-connect/tests/routing.rs`
- Modify: `smelly-connect/tests/transport_stack.rs`

- [ ] **Step 1: Write the failing error-shape and support-module tests**

```rust
#[test]
fn legacy_internal_api_is_not_public() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/legacy_public_api.rs");
}

#[test]
fn facade_error_preserves_typed_source_categories() {
    let _ = std::any::TypeId::of::<smelly_connect::Error>();
}
```

```rust
// smelly-connect/tests/ui/legacy_public_api.rs
use smelly_connect::auth::control::{request_ip_via_tunnel_with_conn, run_control_plane};
use smelly_connect::session::EasyConnectSession;

fn main() {
    let _ = run_control_plane;
    let _ = request_ip_via_tunnel_with_conn;
    let _ = EasyConnectSession::with_legacy_data_plane;
    let _ = EasyConnectSession::spawn_packet_device;
}
```

- [ ] **Step 2: Run the focused and existing tests to verify failure**

Run: `cargo test -p smelly-connect --test public_api_audit --test auth_login --test connect_control_plane --test transport_stack`
Expected: FAIL with public legacy symbols still exported, missing `trybuild`, or outdated error variants

- [ ] **Step 3: Introduce layered errors, move harnesses out of production module bodies, and remove forbidden public symbols**

```rust
[dev-dependencies]
trybuild = "1"

pub enum Error {
    ControlPlane(ControlPlaneError),
    TunnelBootstrap(TunnelBootstrapError),
    RouteDecision(RouteDecisionError),
    Transport(TransportError),
    Integration(IntegrationError),
}
```

- [ ] **Step 4: Rerun the focused regression tests after the cleanup**

Run: `cargo test -p smelly-connect --test public_api_audit --test auth_login --test connect_control_plane --test transport_stack`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/Cargo.toml smelly-connect/src/test_support/mod.rs smelly-connect/src/error.rs smelly-connect/src/auth/control.rs smelly-connect/src/auth/mod.rs smelly-connect/src/protocol/mod.rs smelly-connect/src/session.rs smelly-connect/src/transport/mod.rs smelly-connect/src/integration/mod.rs smelly-connect/src/integration/http_proxy.rs smelly-connect/src/proxy/mod.rs smelly-connect/src/lib.rs smelly-connect/tests/public_api_audit.rs smelly-connect/tests/ui/legacy_public_api.rs smelly-connect/tests/auth_login.rs smelly-connect/tests/connect_control_plane.rs smelly-connect/tests/http_proxy.rs smelly-connect/tests/protocol_tls.rs smelly-connect/tests/reqwest_integration.rs smelly-connect/tests/resolver.rs smelly-connect/tests/resource_parse.rs smelly-connect/tests/routing.rs smelly-connect/tests/transport_stack.rs
git commit -m "refactor: remove legacy surface and tighten errors"
```

## Task 9: Finalize Metadata, Docs, And Full Verification For v0.2.0

**Files:**
- Modify: `smelly-connect/Cargo.toml`
- Modify: `README.md`
- Modify: `smelly-connect/README.md`
- Modify: `smelly-connect/src/main.rs`

- [ ] **Step 1: Write the failing metadata assertion**

```rust
#[test]
fn crate_version_is_0_2_0() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.2.0");
}
```

- [ ] **Step 2: Run the metadata check before the version bump**

Run: `cargo test -p smelly-connect crate_version_is_0_2_0 -- --exact`
Expected: FAIL because the package version is still `0.1.0`

- [ ] **Step 3: Update package metadata, docs, and CLI messaging**

```toml
[package]
name = "smelly-connect"
version = "0.2.0"
edition = "2024"
```

- [ ] **Step 4: Run the full verification suite**

Run: `cargo fmt --all --check`
Expected: PASS

Run: `cargo test --workspace`
Expected: PASS

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS or a short actionable list that must be fixed before completion

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/Cargo.toml README.md smelly-connect/README.md smelly-connect/src/main.rs
git commit -m "release: finalize smelly-connect v0.2.0"
```

## Notes For The Implementer

- Do not introduce parallel old/new implementations that drift apart. Migrate once, then delete or reduce old paths to shallow compatibility layers.
- Keep the public Tokio-based async contract stable: `VpnStream` remains `AsyncRead + AsyncWrite + Send + Unpin + 'static`.
- If any step reveals that `domain::Session` should be a trait plus façade-owned concrete type, update the plan and spec together before continuing. Do not silently improvise a conflicting structure.
- Do not move to `0.2.0` metadata until the redesign code and tests are complete.
