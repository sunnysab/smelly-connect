# smelly-connect-cli Pool Resilience Design

- Date: 2026-03-26
- Status: Implemented
- Scope: Add resilient pool behavior to `smelly-connect-cli`, including failure thresholds, circuit-breaker-style node states, exponential backoff, and protocol-specific fast-fail behavior when there is no usable upstream
- Applies To: `smelly-connect-cli` only

## Goal

Improve the `smelly-connect-cli` account pool so it behaves predictably and safely when VPN sessions or upstream connectivity become unavailable.

The design must support:

- round-robin request distribution across healthy nodes
- consecutive-failure tracking per account node
- configurable failure threshold with default `3`
- node removal from rotation only after the threshold is reached
- exponential backoff with configuration knobs
- request-triggered early recovery attempts when the pool has no currently healthy node
- fast-fail request behavior when there is still no usable upstream
- protocol-specific failure semantics:
  - HTTP returns `503 Service Unavailable`
  - SOCKS5 returns `network unreachable`
- process survival even when the entire pool is temporarily unavailable

## Non-Goals

This design does not include:

- weighted balancing
- sticky routing
- automatic process restart
- changing `smelly-connect` library semantics to embed a generic circuit breaker
- persistent node health state across process restarts

## Problem Statement

The current pool is round-robin oriented but not resilient enough for real operations.

Problems to solve:

- one account may fail repeatedly and continue harming request latency if it stays in rotation forever
- the whole school VPN may become unavailable at night, causing all sessions to fail
- requests need deterministic and protocol-correct failure behavior when no upstream is available
- the CLI should continue running and recover when possible instead of exiting on transient pool exhaustion

## Design Principles

1. Resilience remains a CLI application concern.
2. Normal traffic should only flow through nodes currently considered selectable.
3. A node is not immediately ejected for a single failure; it should be removed only after the configured threshold.
4. When the pool is exhausted, requests should fail quickly and clearly.
5. Recovery should be possible both by timer and by request-triggered re-entry attempts.
6. The process should survive temporary total upstream unavailability.

## Node State Model

Each account node should move through explicit states:

- `Ready`
- `Suspect`
- `Open`
- `HalfOpen`

### `Ready`

- participates in round-robin
- failure counter is below threshold or fully reset

### `Suspect`

- the node has started accumulating consecutive failures
- it has not yet reached the failure threshold
- it still participates in normal round-robin until the configured threshold is reached
- it is therefore still treated as selectable traffic-bearing capacity, but its failure counter remains active

### `Open`

- the node is removed from normal rotation
- it remains unavailable until `open_until`
- it carries the latest error and backoff state

### `HalfOpen`

- the node is allowed exactly one recovery attempt
- success returns it to `Ready`
- failure returns it to `Open` with a new backoff window

## Failure Counters

Per-node fields should include at least:

- `consecutive_failures`
- `failure_threshold`
- `last_error`
- `open_until`
- `current_backoff`

Default behavior:

- `failure_threshold = 3`

This value must be configurable in `config.toml`.

## Configuration

Extend the CLI pool config with:

```toml
[pool]
prewarm = 2
connect_timeout_secs = 20
healthcheck_interval_secs = 60
selection = "round_robin"

failure_threshold = 3
backoff_base_secs = 30
backoff_max_secs = 600
allow_request_triggered_probe = true
```

### Meaning

- `failure_threshold`
  Number of consecutive failures before a node is removed from rotation.
- `backoff_base_secs`
  Initial delay used for exponential retry after the node enters `Open`.
- `backoff_max_secs`
  Upper bound for backoff delay.
- `allow_request_triggered_probe`
  When the pool has no ready node, allow a request to attempt an early recovery probe.

## Backoff Behavior

Backoff should be exponential:

- first open interval starts at `backoff_base_secs`
- subsequent failures grow exponentially
- capped at `backoff_max_secs`

The implementation does not need persistent backoff memory across process restarts.

## Request Routing Rules

For this design:

- selectable nodes = `Ready` or `Suspect`
- non-selectable nodes = `Open` or `HalfOpen`

### Normal Case

When at least one node is selectable:

- choose among `Ready` and `Suspect` nodes using round-robin
- do not route normal traffic through `Open` nodes
- `HalfOpen` is not part of normal round-robin selection

### Failure Escalation

When a request routed through a node fails:

- increment `consecutive_failures`
- if below threshold, keep the node available but marked as suspect
- once threshold is reached, move it to `Open`

For this design, a request failure means one of:

- upstream connect failure
- upstream handshake failure
- upstream request forwarding failure before the connection is established
- proxy-side failure that proves the selected upstream session is unusable

It does not include:

- ordinary downstream client disconnect after a connection was already established
- successful upstream responses such as HTTP `5xx`
- expected client-side cancellation after a tunnel is already active

### Full Pool Exhaustion

If no selectable node exists:

- check whether any `Open` node is eligible to re-enter via timer
- if none is timer-eligible but `allow_request_triggered_probe = true`, allow one early recovery attempt
- if recovery succeeds, serve the request with that node
- if recovery fails or no probe is possible, fail the request immediately

Concurrency rule:

- at most one `HalfOpen` attempt may exist per node at a time
- when the pool is exhausted, a single request may nominate at most one node for recovery
- concurrent requests must not trigger multiple simultaneous `HalfOpen` attempts on the same node
- if one request is already probing a node, other concurrent requests must fail fast using the protocol-specific upstream-unavailable response

## Protocol-Specific Fast-Fail Behavior

### HTTP

If the pool cannot provide a usable upstream session:

- return `503 Service Unavailable`
- include a small body such as `no upstream session available` if helpful

### SOCKS5

If the pool cannot provide a usable upstream session:

- return SOCKS5 reply code `network unreachable` (`0x03`)

This is intentional and should be treated as a stable contract for this feature.

## Whole-VPN-Outage Behavior

When the school VPN is unavailable and all nodes are effectively unusable:

- the process must remain alive
- listeners remain bound
- incoming requests fail fast using the protocol-specific rules above
- background retry/backoff continues
- request-triggered probe may attempt an early recovery when enabled

The CLI should not exit merely because the pool is temporarily empty.

## Logging Expectations

This feature should emit:

### `info`

- node recovery success
- request assignment to a ready node

### `warn`

- node failure count increments
- node moved to `Open`
- request fast-fail due to no available upstream
- request-triggered probe failed

### `error`

- unexpected internal errors during recovery orchestration

## Testing Requirements

Implementation planning must include tests for:

- single failure below threshold does not immediately eject the node
- threshold crossing removes the node from rotation
- only `Ready` and `Suspect` nodes participate in normal round-robin
- exponential backoff grows and respects the configured max
- request-triggered probe can recover a node when enabled
- HTTP returns `503` when there is no usable upstream
- SOCKS5 returns `network unreachable` when there is no usable upstream
- process/service remains alive when the entire pool is temporarily unavailable
- node recovery success returns it to rotation

## Acceptance Criteria

The resilience feature is complete when:

1. Node failure tracking is per account.
2. Nodes are removed only after the configured consecutive-failure threshold.
3. Exponential backoff is configurable and capped.
4. Normal routing only uses healthy nodes.
   Here, “healthy” means selectable (`Ready` or `Suspect`) in this design.
5. Request-triggered recovery is possible when configured.
6. HTTP returns `503 Service Unavailable` when no upstream is usable.
7. SOCKS5 returns `network unreachable` when no upstream is usable.
8. The proxy process remains alive during temporary total upstream outages.
9. Recovered nodes re-enter rotation cleanly.
