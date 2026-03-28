# Code Review Remediation Design

- Date: 2026-03-28
- Status: Proposed
- Scope: `smelly-connect`, `smelly-connect-cli`, and `smelly-tls`
- Applies To: workspace-wide runtime, routing, API boundary, and maintainability remediation

## Goal

Fix the highest-value issues found during code review without breaking the currently verified EasyConnect control-plane and proxy behavior.

This remediation should:

- stop leaking proxy listeners and detached background tasks
- make route semantics match configuration, including protocol-aware matching
- make runtime health/status reporting truthful
- raise concurrency and throughput ceilings for the CLI proxy
- make session and pool hot paths cheaper
- reduce public API leakage and test-only code pollution
- leave the workspace green under targeted tests and full workspace verification

## Scope Check

The review findings span multiple independent subsystems:

- proxy resource ownership
- CLI runtime/supervision
- routing semantics
- session and pool hot paths
- module/API boundaries
- protocol-stack hygiene

They could be split into separate plans. For execution speed, this document keeps them in one coordinated remediation program, but every task in the paired implementation plan must remain independently shippable.

## Non-Goals

This remediation does not attempt to:

- redesign EasyConnect protocol support beyond correctness and cleanup
- add weighted load balancing or sticky sessions
- add generic plugin systems or configurable middleware
- add HTTP/2 or HTTP/3 proxy support
- rewrite `smelly-tls` into a fully general TLS library

## Requirements

### 1. Proxy Resource Ownership

- `smelly_connect::Session::reqwest_client()` must no longer leak a proxy listener or detached task per client construction.
- proxy/listener lifetime must be tied to an owned Rust value that naturally drops cleanly
- library proxy handles should clean up on drop even when callers forget explicit shutdown

### 2. Listener Supervision And Runtime Model

- `smelly-connect-cli` must not hide listener startup or runtime failures behind fire-and-forget task spawning
- if any required listener crashes, the process must surface the failure to the top-level command
- the CLI runtime should use a multi-thread executor for long-running proxy workloads
- listener task fan-out should gain an explicit concurrency limit or equivalent backpressure

### 3. Route Semantics

- route evaluation must honor configured protocol values instead of ignoring them
- domain and IP rule matching must come from one shared implementation
- route matching should avoid unnecessary per-rule allocation in the hot path
- invalid or unsupported protocol values should fail configuration load or route construction deterministically

### 4. Runtime Status Correctness

- connection failures that matter for recovery/status should increment runtime failure accounting
- successful connect paths should continue to clear recovery state
- management and status endpoints should reflect real health transitions instead of dead metrics

### 5. Session And Pool Hot Paths

- cloning a live session should be cheap and should not deep-clone route tables and DNS maps
- pool selection and health flows should stop paying repeated whole-state scans where simple indexes or smaller state units suffice
- prewarm and periodic healthcheck work should allow bounded concurrency rather than strict serialization
- duplicated account/session data in pool state should be removed or reduced

### 6. HTTP Proxy Throughput

- upstream HTTP forwarding should stop forcing `Connection: close` on every request when reuse is safe
- keep-alive support should preserve current correctness around `CONNECT`, `HEAD`, chunked bodies, and oversized headers

### 7. API Boundary And Test Isolation

- internal modules should stop leaking as de facto public API unless intentionally supported
- `test_support` and other test harness code should not compile into ordinary debug binaries
- oversized modules should be split by responsibility so production code and test harness code are separate

### 8. Error And Protocol Hygiene

- CLI command/runtime layers should replace broad `Result<_, String>` boundaries with structured error types
- production `unwrap`/`expect` in protocol and transport code should be reduced where realistic failures can be surfaced cleanly
- names that imply generic behavior but actually implement IPv4-only behavior should be tightened or documented via renaming

## Constraints

- keep current passing workspace behavior unless a reviewed finding explicitly requires behavior change
- use TDD for each remediation task
- prefer additive compatibility where possible; if an API change is required, pair it with a migration-focused test and README/doc update
- preserve current real-world EasyConnect login and tunnel bootstrap coverage

## Validation

The remediation is complete when:

- targeted regression tests for each reviewed issue exist and pass
- `cargo test --workspace` passes
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- no known issue from the remediation list remains untracked
