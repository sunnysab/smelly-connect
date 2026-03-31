# Routing Default Direct Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add configurable unmatched-route behavior so custom domain/IP rules still force VPN routing, while unmatched targets default to local direct connections and can be switched to explicit blocking.

**Architecture:** Extend the library route planner to produce explicit VPN/direct outcomes and keep route denial as a policy decision. Thread the new routing default action from CLI config into session construction, then teach HTTP and SOCKS5 proxy layers to execute either VPN or direct connections from the same route plan instead of hardcoding VPN-only behavior.

**Tech Stack:** Rust stable, Tokio, Serde/TOML, `smelly-connect`, `smelly-connect-cli`, Hyper, fast-socks5, `cargo test`, `cargo clippy`

---

## File Map

- Modify: `smelly-connect/src/domain/route_policy.rs`
- Modify: `smelly-connect/src/domain/mod.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/lib.rs`
- Modify: `smelly-connect/tests/routing.rs`
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/src/commands/inspect.rs`
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/tests/cli_config.rs`
- Modify: `smelly-connect-cli/tests/inspect.rs`
- Modify: `smelly-connect-cli/tests/http_proxy.rs`
- Modify: `smelly-connect-cli/tests/socks5_proxy.rs`
- Modify: `config.toml.example`
- Modify: `README.md`

### Task 1: Add Routing Default Action Config Parsing

**Files:**
- Modify: `smelly-connect-cli/src/config.rs`
- Modify: `smelly-connect-cli/tests/cli_config.rs`
- Modify: `smelly-connect-cli/tests/fixtures/config.sample.toml`

- [ ] **Step 1: Write the failing config tests**

Add tests in `smelly-connect-cli/tests/cli_config.rs` for:

```rust
#[test]
fn routing_default_action_defaults_to_direct() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(/* minimal config */).unwrap();
    assert_eq!(
        cfg.routing.default_action,
        smelly_connect_cli::config::RoutingDefaultAction::Direct
    );
}

#[test]
fn parses_block_default_action_from_config() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(/* config with block */).unwrap();
    assert_eq!(
        cfg.routing.default_action,
        smelly_connect_cli::config::RoutingDefaultAction::Block
    );
}

#[test]
fn invalid_default_action_is_rejected() {
    let cfg = toml::from_str::<smelly_connect_cli::config::AppConfig>(/* invalid action */);
    assert!(cfg.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p smelly-connect-cli --test cli_config routing_default_action_defaults_to_direct -- --exact`

Expected: FAIL with missing `default_action` field or missing enum/type support

- [ ] **Step 3: Write minimal implementation**

Add a new config enum in `smelly-connect-cli/src/config.rs`:

```rust
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RoutingDefaultAction {
    #[default]
    Direct,
    Block,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RoutingConfig {
    pub allow_all: bool,
    pub default_action: RoutingDefaultAction,
    pub domain_rules: Vec<LocalDomainRuleConfig>,
    pub ip_rules: Vec<LocalIpRuleConfig>,
}
```

Update the sample fixture config to include:

```toml
[routing]
allow_all = false
default_action = "direct"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p smelly-connect-cli --test cli_config`

Expected: PASS with the new routing config parsing tests green

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/config.rs smelly-connect-cli/tests/cli_config.rs smelly-connect-cli/tests/fixtures/config.sample.toml
git commit -m "feat(cli): parse routing default action"
```

### Task 2: Extend Library Route Planning To Return Direct Outcomes

**Files:**
- Modify: `smelly-connect/src/domain/route_policy.rs`
- Modify: `smelly-connect/src/domain/mod.rs`
- Modify: `smelly-connect/src/session.rs`
- Modify: `smelly-connect/src/lib.rs`
- Modify: `smelly-connect/tests/routing.rs`

- [ ] **Step 1: Write the failing library routing tests**

Add focused tests in `smelly-connect/tests/routing.rs` for:

```rust
#[tokio::test]
async fn routing_returns_direct_for_unmatched_targets_by_default() {
    let session = smelly_connect::test_support::session::fake_session_without_match();
    let route = session.plan_tcp_connect(("example.com", 443)).await.unwrap();
    assert!(matches!(route, smelly_connect::session::RoutePlan::Direct(_)));
}

#[tokio::test]
async fn routing_blocks_unmatched_targets_when_policy_is_block() {
    let session = smelly_connect::test_support::session::fake_session_without_match()
        .with_route_policy(smelly_connect::domain::route_policy::RoutePolicy::block_non_resource_targets());
    let err = session.plan_tcp_connect(("example.com", 443)).await.unwrap_err();
    assert!(matches!(err, smelly_connect::Error::RouteDecision(_)));
}
```

Retain and update existing allow-all coverage so it still expects `VpnResolved`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p smelly-connect --test routing routing_returns_direct_for_unmatched_targets_by_default -- --exact`

Expected: FAIL because unmatched routes still return `TargetNotAllowed`

- [ ] **Step 3: Write minimal implementation**

Refactor the route model in `smelly-connect/src/session.rs` and `smelly-connect/src/domain/route_policy.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutePolicy {
    #[default]
    DirectNonResourceTargets,
    RejectNonResourceTargets,
}

impl RoutePolicy {
    pub fn direct_non_resource_targets() -> Self {
        Self::DirectNonResourceTargets
    }

    pub fn block_non_resource_targets() -> Self {
        Self::RejectNonResourceTargets
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePlan {
    VpnResolved(SocketAddr),
    Direct(SocketAddr),
}
```

Extend `EasyConnectSession` to carry `route_policy`, add:

```rust
pub fn with_route_policy(mut self, route_policy: RoutePolicy) -> Self
```

Change `plan_tcp_connect` and the private planning helpers so they:

- return `VpnResolved` when `allow_all` is true or when resource/local rules match
- return `Direct(SocketAddr)` when unmatched and `route_policy` is direct
- return `TargetNotAllowed` when unmatched and `route_policy` is block

Keep `SessionUdpSocket::send_to` temporarily unchanged in this task; UDP direct support lands in Task 5.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p smelly-connect --test routing`

Expected: PASS with updated direct/block/allow-all route planning

- [ ] **Step 5: Commit**

```bash
git add smelly-connect/src/domain/route_policy.rs smelly-connect/src/domain/mod.rs smelly-connect/src/session.rs smelly-connect/src/lib.rs smelly-connect/tests/routing.rs
git commit -m "feat(core): add direct route planning policy"
```

### Task 3: Thread Routing Policy From CLI Config Into Session Construction

**Files:**
- Modify: `smelly-connect-cli/src/pool.rs`
- Modify: `smelly-connect-cli/src/commands/inspect.rs`
- Modify: `smelly-connect-cli/tests/inspect.rs`

- [ ] **Step 1: Write the failing CLI inspection tests**

Add tests in `smelly-connect-cli/tests/inspect.rs` for:

```rust
#[tokio::test]
async fn inspect_route_reports_direct_for_unmatched_targets() {
    let output = smelly_connect_cli::commands::inspect::run_route_with_config(
        "tests/fixtures/config.sample.toml",
        "example.com",
        443,
    ).await.unwrap();
    assert!(output.contains("Direct("));
}
```

Add or update a second fixture-backed test for block mode if needed.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p smelly-connect-cli --test inspect inspect_route_reports_direct_for_unmatched_targets -- --exact`

Expected: FAIL because sessions are still created without route-policy wiring

- [ ] **Step 3: Write minimal implementation**

In `smelly-connect-cli/src/pool.rs`, derive the library route policy from CLI config:

```rust
let route_policy = match cfg.routing.default_action {
    RoutingDefaultAction::Direct => smelly_connect::domain::route_policy::RoutePolicy::direct_non_resource_targets(),
    RoutingDefaultAction::Block => smelly_connect::domain::route_policy::RoutePolicy::block_non_resource_targets(),
};
```

Apply it during session construction:

```rust
let session = session
    .with_local_route_overrides(local_route_overrides.clone())
    .with_route_policy(route_policy)
    .with_allow_all_routes(allow_all_routes);
```

Update `inspect` tests or helper text only as needed to preserve readable output.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p smelly-connect-cli --test inspect`

Expected: PASS with direct route inspection visible

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/pool.rs smelly-connect-cli/src/commands/inspect.rs smelly-connect-cli/tests/inspect.rs
git commit -m "feat(cli): wire routing policy into sessions"
```

### Task 4: Add Direct TCP Handling To The HTTP Proxy

**Files:**
- Modify: `smelly-connect-cli/src/proxy/http.rs`
- Modify: `smelly-connect-cli/tests/http_proxy.rs`

- [ ] **Step 1: Write the failing HTTP proxy tests**

Add focused tests in `smelly-connect-cli/tests/http_proxy.rs` to prove unmatched routes now connect directly:

```rust
#[tokio::test]
async fn http_proxy_connect_uses_direct_route_for_unmatched_targets() {
    let result = smelly_connect_cli::proxy::http::/* helper */().await.unwrap();
    assert_eq!(result.status_line, "HTTP/1.1 200 OK");
}

#[tokio::test]
async fn http_proxy_forward_uses_direct_route_for_unmatched_targets() {
    let result = smelly_connect_cli::proxy::http::/* helper */().await.unwrap();
    assert_eq!(result.status_line, "HTTP/1.1 200 OK");
}
```

Also add block-mode coverage that still returns `403` where appropriate.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p smelly-connect-cli --test http_proxy http_proxy_connect_uses_direct_route_for_unmatched_targets -- --exact`

Expected: FAIL because unmatched targets are still treated as rejected

- [ ] **Step 3: Write minimal implementation**

In `smelly-connect-cli/src/proxy/http.rs`, route before connecting:

```rust
match session.plan_tcp_connect((host.as_str(), port)).await {
    Ok(smelly_connect::session::RoutePlan::VpnResolved(addr)) => {
        // existing VPN path
    }
    Ok(smelly_connect::session::RoutePlan::Direct(addr)) => {
        // Tokio TcpStream::connect(addr)
    }
    Err(smelly_connect::Error::RouteDecision(
        smelly_connect::error::RouteDecisionError::TargetNotAllowed,
    )) => {
        // existing forbidden path
    }
    Err(err) => {
        // existing failure path
    }
}
```

Keep logging and stats shared; only the connection backend should differ.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p smelly-connect-cli --test http_proxy`

Expected: PASS with both direct and blocked behaviors covered

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/proxy/http.rs smelly-connect-cli/tests/http_proxy.rs
git commit -m "feat(http-proxy): support direct unmatched routes"
```

### Task 5: Add Direct TCP And UDP Handling To SOCKS5

**Files:**
- Modify: `smelly-connect-cli/src/proxy/socks5.rs`
- Modify: `smelly-connect-cli/tests/socks5_proxy.rs`

- [ ] **Step 1: Write the failing SOCKS5 tests**

Add tests for:

```rust
#[tokio::test]
async fn socks5_connect_uses_direct_route_for_unmatched_targets() {
    let result = smelly_connect_cli::proxy::socks5::/* helper */().await.unwrap();
    assert_eq!(result.reply_code, 0x00);
}

#[tokio::test]
async fn socks5_udp_associate_uses_direct_route_for_unmatched_targets() {
    let result = smelly_connect_cli::proxy::socks5::/* helper */().await.unwrap();
    assert_eq!(result.echoed_bytes, b"ping");
}
```

Add a block-mode test that still returns `ReplyError::ConnectionNotAllowed`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p smelly-connect-cli --test socks5_proxy socks5_connect_uses_direct_route_for_unmatched_targets -- --exact`

Expected: FAIL because SOCKS5 only knows how to use VPN transport today

- [ ] **Step 3: Write minimal implementation**

For TCP CONNECT in `smelly-connect-cli/src/proxy/socks5.rs`:

```rust
match session.plan_tcp_connect((host.as_str(), port)).await {
    Ok(smelly_connect::session::RoutePlan::VpnResolved(addr)) => {
        // existing session transport connect
    }
    Ok(smelly_connect::session::RoutePlan::Direct(addr)) => {
        // TcpStream::connect(addr)
    }
    Err(smelly_connect::Error::RouteDecision(
        smelly_connect::error::RouteDecisionError::TargetNotAllowed,
    )) => {
        // map to ConnectionNotAllowed
    }
    Err(_) => {
        // existing failure path
    }
}
```

For UDP associate, introduce an internal outbound enum such as:

```rust
enum UdpOutbound {
    Vpn(smelly_connect::session::SessionUdpSocket),
    Direct(UdpSocket),
}
```

Route each datagram target and dispatch through the matching backend.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p smelly-connect-cli --test socks5_proxy`

Expected: PASS with TCP direct, UDP direct, and blocked reply behavior covered

- [ ] **Step 5: Commit**

```bash
git add smelly-connect-cli/src/proxy/socks5.rs smelly-connect-cli/tests/socks5_proxy.rs
git commit -m "feat(socks5): support direct unmatched routes"
```

### Task 6: Update User-Facing Docs And Verify The Whole Change

**Files:**
- Modify: `config.toml.example`
- Modify: `README.md`

- [ ] **Step 1: Write the failing doc/config expectations**

Add or update existing fixture assertions so the checked sample config includes:

```toml
[routing]
allow_all = false
default_action = "direct"
```

If no automated doc checks exist, record the exact text to add before editing.

- [ ] **Step 2: Run verification that shows the gap**

Run: `cargo test -p smelly-connect-cli --test cli_config parses_sample_config -- --exact`

Expected: FAIL if the sample config/fixture is still missing the documented routing field

- [ ] **Step 3: Update docs**

Document in both `config.toml.example` and `README.md`:

- `routing.default_action = "direct" | "block"`
- default is `direct`
- `allow_all = true` still forces VPN for everything
- `default_action = "block"` restores the previous reject-on-miss behavior

- [ ] **Step 4: Run full verification**

Run: `cargo test -p smelly-connect --test routing`

Expected: PASS

Run: `cargo test -p smelly-connect-cli --test cli_config --test inspect --test http_proxy --test socks5_proxy`

Expected: PASS

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add config.toml.example README.md
git commit -m "docs: document routing default action"
```
