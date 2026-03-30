# Routing Default Direct Design

**Date:** 2026-03-30

**Goal:** Allow custom local domain/IP VPN rules while making unmatched targets configurable as either `direct` or `block`, with `direct` as the default behavior.

## Context

Current routing behavior inside `smelly-connect` is binary:

- A target that matches server resources or local overrides is routed through the VPN.
- A target that does not match is rejected with `TargetNotAllowed`.

This is too restrictive for proxy usage. The user wants local custom domain/IP rules to stay available, but unmatched traffic should no longer be forced into rejection. Instead, unmatched traffic should be configurable as either:

- `direct`: use the local machine network stack
- `block`: explicitly reject the target

The default should be `direct`.

## Requirements

### Functional

- Keep support for local custom `routing.domain_rules` and `routing.ip_rules`.
- Preserve existing `allow_all = true` behavior as an explicit "send everything through VPN" override.
- Introduce a routing default action for unmatched traffic.
- Make unmatched behavior configurable as `direct` or `block`.
- Default unmatched behavior to `direct` when config does not specify it.
- Keep route decision semantics consistent across:
  - library route planning
  - CLI inspect output
  - HTTP proxy
  - SOCKS5 TCP connect
  - SOCKS5 UDP associate

### Non-Functional

- Avoid ad hoc direct-connect logic in only one proxy implementation.
- Keep route precedence easy to explain.
- Preserve current VPN-only behavior for explicitly matched rules.
- Add coverage for configuration parsing, route planning, and proxy execution.

## Route Model

Routing should move from a binary "VPN or reject" model to an explicit three-way decision:

- `VpnResolved(SocketAddr)`: target should use the VPN transport after normal resolution.
- `Direct(SocketAddr)`: target should connect using the local OS network stack.
- `Blocked`: target is intentionally denied by routing policy.

The route planner should remain the source of truth for this decision. Proxy layers should consume route plans rather than duplicating routing policy.

## Routing Precedence

Route evaluation order:

1. If `allow_all = true`, route every target through the VPN.
2. Otherwise, check server resource rules.
3. Then check local custom domain/IP overrides.
4. If still unmatched, use `default_action`.

Behavior of `default_action`:

- `direct`: return a direct route plan
- `block`: return a blocked route plan or an equivalent route-decision error for explicit denial

This preserves one simple mental model:

- matched rules mean "VPN"
- unmatched policy decides "direct or block"
- `allow_all` means "ignore that distinction and always try VPN"

## Configuration Design

`[routing]` gains a new field:

```toml
[routing]
allow_all = false
default_action = "direct"
```

Supported values:

- `direct`
- `block`

Defaults:

- `allow_all = false`
- `default_action = "direct"`

Local route tables remain unchanged:

```toml
[[routing.domain_rules]]
domain = "*.foo.edu.cn"
port_min = 443
port_max = 443
protocol = "tcp"

[[routing.ip_rules]]
ip_min = "42.62.107.1"
ip_max = "42.62.107.254"
port_min = 1
port_max = 65535
protocol = "all"
```

Compatibility notes:

- Existing configs without `default_action` will now default to `direct`.
- Existing configs with `allow_all = true` keep current semantics.
- Users who want previous rejection behavior must explicitly set `default_action = "block"`.

## Library Changes

### `smelly-connect`

Introduce an explicit routing policy type that captures unmatched behavior. The session should carry this policy so it can be reused by all route-planning entry points.

Planned library changes:

- Expand `RoutePolicy` from a single-value enum to a meaningful policy model.
- Expand `RoutePlan` to represent at least VPN and direct outcomes.
- Update `plan_tcp_connect` and related UDP route decisions to return policy-driven outcomes.
- Preserve `allow_all` as the highest-priority compatibility override.
- Keep existing domain/IP rule matching logic unchanged where possible.

Blocked behavior can remain an error if that leads to less churn in the public API, but direct behavior must be represented explicitly in route planning.

## Proxy Execution Design

### HTTP proxy

For both CONNECT and forward proxy paths:

- `VpnResolved` continues to use `session.connect_tcp(...)`.
- `Direct` uses Tokio `TcpStream::connect(...)` against the resolved target.
- `Blocked` maps to the existing route-rejected response path.

Connection metrics and logs should remain shared, with only the transport backend changing.

### SOCKS5 TCP

For CONNECT:

- `VpnResolved` uses the current VPN transport.
- `Direct` opens a normal Tokio TCP connection.
- `Blocked` maps to `connection not allowed`.

### SOCKS5 UDP

Current UDP associate behavior assumes VPN-backed UDP only. This needs a second execution path:

- `VpnResolved` keeps using `SessionUdpSocket`.
- `Direct` uses a local Tokio `UdpSocket`.
- `Blocked` drops or rejects according to current SOCKS5 behavior for denied targets.

The UDP routing decision must be made per datagram target so mixed destinations behave correctly.

## CLI Behavior

### `inspect route`

Route inspection should expose the actual route decision instead of collapsing everything into allowed/rejected VPN-only semantics.

Expected shape:

- VPN match: `allowed: VpnResolved(...)`
- unmatched + direct: `allowed: Direct(...)`
- unmatched + block: `rejected: RouteDecision(TargetNotAllowed)` or equivalent blocked result

### `routes`

No new route-table shape is required for custom rules. If useful, management snapshots may include the configured unmatched default action, but this is optional for the first iteration.

## Error Semantics

Error handling should distinguish between:

- explicit route denial (`block`)
- connection failure on a direct route
- connection failure on a VPN route

Direct routing is not an error by itself. Only the direct connection attempt may fail.

This avoids overloading `TargetNotAllowed` to mean both "policy denied" and "use local network".

## Testing Strategy

### Config parsing

- `default_action` defaults to `direct`
- `default_action = "block"` parses successfully
- invalid `default_action` values are rejected

### Library routing

- unmatched target returns direct route by default
- unmatched target returns block/rejection when configured
- matched local domain rule still routes via VPN
- matched local IP rule still routes via VPN
- `allow_all = true` still routes unmatched targets via VPN

### CLI inspect

- inspect reports `Direct(...)` for unmatched targets under default config
- inspect reports rejection when `default_action = "block"`

### HTTP proxy

- unmatched target reaches a local test upstream via direct mode
- blocked unmatched target returns the expected gateway refusal response
- matched target behavior is unchanged

### SOCKS5 proxy

- unmatched TCP CONNECT reaches a local test upstream via direct mode
- unmatched UDP ASSOCIATE datagrams reach a local UDP echo upstream via direct mode
- blocked unmatched target still maps to SOCKS5 denial

## Risks And Tradeoffs

- Adding `Direct` to route planning broadens the route model and touches multiple layers, but it keeps semantics coherent.
- SOCKS5 UDP is the highest-risk area because it currently assumes VPN-backed UDP state.
- Defaulting old configs to `direct` changes behavior for users who relied on implicit blocking; this is intentional and must be documented clearly.

## Rollout Notes

- Update `config.toml.example` and README to document `routing.default_action`.
- Keep `allow_all` documented as a stronger compatibility override.
- Mention explicitly that to preserve old reject-on-miss behavior, users must set `default_action = "block"`.
