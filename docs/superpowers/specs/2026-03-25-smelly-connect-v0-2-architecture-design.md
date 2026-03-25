# smelly-connect v0.2 Architecture Design

- Date: 2026-03-25
- Status: Reviewed draft
- Scope: Architecture redesign of the current `smelly-connect` workspace into a long-lived library with a protocol-kernel-centered structure
- Baseline: The current codebase should be preserved as `git tag v0.1` before implementation starts
- Target Version: After the redesign is complete, the crate version should move from `0.1.x` behavior to `0.2.0`

## Goal

Redesign `smelly-connect` as a maintainable library with clear architecture boundaries, stable public APIs, and explicit ownership of runtime resources.

The redesign must optimize for:

- long-term maintainability rather than short-term convenience
- clear module boundaries and one-way dependency flow
- stable public library semantics that hide protocol and runtime internals
- protocol correctness for the already-fixed EasyConnect protocol
- explicit lifecycle management for background tasks and integrations

## Non-Goals

This redesign does not aim to:

- add new end-user protocol features
- broaden platform support beyond the current Linux-first scope
- add non-EasyConnect protocol support
- introduce speculative abstraction for future unknown protocols
- preserve internal module layout or every current public API exactly as-is

## Current Problems To Fix

The current codebase is functionally working, but several design problems make it a weak long-term library foundation:

1. `EasyConnectSession` currently mixes domain state, routing policy, runtime transport, keepalive control, integration entrypoints, and protocol bootstrap details.
2. `EasyConnectConfig` mixes user configuration, testing hooks, bootstrap orchestration, and runtime behavior selection.
3. `auth::control` is a god module that currently combines control-plane HTTP, login flow, token extraction, tunnel setup, packet device construction, and task spawning.
4. Background tasks are spawned opportunistically without a unified owner or shutdown model.
5. Public APIs such as `reqwest_client()` currently hide significant runtime behavior and resource ownership.
6. Error types flatten too much context into string payloads, which blocks structured recovery and precise testing.
7. Module layout follows source grouping more than stable architectural boundaries.

## Design Principles

The redesign must follow these hard rules:

1. Public library APIs expose library semantics, not implementation choreography.
2. Protocol types and parsers stay pure whenever possible.
3. Runtime orchestration owns I/O, async tasks, and third-party integrations.
4. Domain-layer types do not depend on protocol implementation details.
5. Background services must always have an explicit owner and shutdown surface.
6. Stable capability boundaries use traits only where there is real substitution value.
7. Functions should accept typed context objects instead of repeated loose parameter bundles.

## Target Architecture

The workspace should be reorganized around five layers:

1. `kernel`
2. `runtime`
3. `domain`
4. `integration`
5. `facade`

The intended dependency direction is:

```text
facade -> domain
facade -> integration
facade -> runtime (assembly only)
integration -> domain
runtime -> kernel

domain -/-> runtime
domain -/-> kernel
integration -/-> kernel
```

The important constraint is that the stable public surface must not expose protocol secrets, tunnel primitives, packet devices, or netstack implementation types.

## Layer Responsibilities

### `kernel`

`kernel` contains EasyConnect protocol knowledge and only protocol knowledge.

It should include:

- control-plane message models
- control-plane parsers and encoders
- token derivation
- legacy tunnel handshake message builders and reply parsers
- cipher selection policy and related typed values

It should not include:

- `tokio`
- `reqwest`
- `openssl`
- spawned tasks
- channels
- packet devices
- session orchestration

### `runtime`

`runtime` executes the protocol.

It should include:

- control-plane client implementation
- login flow orchestration
- token acquisition flow
- tunnel establishment
- packet pump orchestration
- `smoltcp`-backed transport runtime
- background task supervision

It may depend on:

- `tokio`
- `reqwest`
- `openssl`
- `smoltcp`
- `smelly-tls`

It should return typed runtime products rather than leaking construction details into higher layers.

### `domain`

`domain` provides the stable vocabulary of the library.

It should include:

- `Session`
- `SessionInfo`
- `ConnectTarget`
- `ConnectPlan`
- `RoutePolicy`
- `Resolver`
- `KeepalivePolicy`
- `ResourceSet`

`Session` should only expose stable capabilities and read-only session information. It must not directly expose derived tokens, tunnel bootstrap materials, raw packet devices, or legacy tunnel handles.

Canonical ownership rule:

- `domain` defines the canonical `Session`, `SessionInfo`, `ConnectTarget`, and related capability-oriented public types.
- `facade` re-exports those types and adds the top-level construction entrypoints such as `EasyConnectClient` and `EasyConnectClientBuilder`.
- `integration` depends on `domain` capability types directly, not on `facade`.

### `integration`

`integration` adapts `Session` capabilities into higher-level libraries and local tooling.

It should include:

- `reqwest` integration
- local HTTP proxy support

It must depend on `domain`-defined `Session` capabilities rather than bypassing the facade into runtime internals.

### `facade`

`facade` is the public entrypoint layer.

It should expose:

- `EasyConnectClient`
- `EasyConnectClientBuilder`
- `Session`
- `SessionInfo`
- `ProxyHandle`
- `KeepaliveHandle`
- `ConnectTarget`

The facade must be small and obvious to users. It should read like a library API, not an implementation wiring surface.

## Target Public API Shape

The public API should converge toward a shape like this:

```rust
let client = EasyConnectClient::builder("vpn1.sit.edu.cn")
    .credentials("user", "pass")
    .captcha_handler(handler)
    .build()?;

let session = client.connect().await?;
let stream = session.connect(("jwxt.sit.edu.cn", 443)).await?;
let proxy = session.start_http_proxy("127.0.0.1:8080".parse()?).await?;
let keepalive = session.start_keepalive("jwxt.sit.edu.cn").await?;
let info = session.info();
```

The stable byte-stream return type should remain a library-owned wrapper such as `VpnStream`.

`VpnStream` requirements:

- implements `tokio::io::AsyncRead`
- implements `tokio::io::AsyncWrite`
- is `Send + Unpin + 'static`
- does not expose raw socket or file-descriptor semantics as part of the stable contract

Key facade/domain capability signatures should converge toward a shape like this:

```rust
pub struct Session;
pub struct SessionInfo;
pub struct VpnStream;
pub struct ProxyHandle;
pub struct KeepaliveHandle;

impl Session {
    pub fn info(&self) -> &SessionInfo;
    pub async fn connect<T>(&self, target: T) -> Result<VpnStream, Error>
    where
        T: Into<ConnectTarget>;

    pub async fn start_http_proxy(&self, bind: std::net::SocketAddr) -> Result<ProxyHandle, Error>;
    pub async fn start_keepalive<T>(&self, target: T) -> Result<KeepaliveHandle, Error>
    where
        T: Into<ConnectTarget>;
}

impl ProxyHandle {
    pub fn local_addr(&self) -> std::net::SocketAddr;
    pub async fn shutdown(self) -> Result<(), Error>;
}

impl KeepaliveHandle {
    pub async fn shutdown(self) -> Result<(), Error>;
}
```

Shutdown semantics:

- `shutdown(self).await` is the explicit graceful-stop path for long-lived services.
- dropping a handle may trigger best-effort cancellation, but graceful stop is only guaranteed through `shutdown`.
- starting a proxy or keepalive loop must never rely on `mem::forget` or detached keepalive tasks for correctness.

The following current concepts should not remain part of the stable public API:

- `with_legacy_data_plane(...)`
- `spawn_packet_device()`
- `request_ip_via_tunnel_with_conn(...)`
- `run_control_plane(...)`

These are internal construction details and belong in runtime or kernel internals.

## Trait Policy

Traits should be introduced only at stable capability boundaries with real substitution value.

The preferred trait set is:

- `ControlPlaneClient`
- `TunnelFactory`
- `NameResolver`
- `Transport`
- `SessionIntegration` if an integration abstraction remains useful after refactoring

The redesign should avoid trait proliferation for plain values. Types such as `DerivedToken`, `ResourceSet`, `ConnectPlan`, and message structs should remain concrete data types.

The current closure-based `TransportStack` is acceptable as an internal testing technique, but it is not the desired long-term core abstraction for the library.

## Async And Task Lifecycle Rules

Async boundaries must reflect real I/O.

Rules:

1. Pure parsing, encoding, matching, and route-decision logic should remain synchronous.
2. Only I/O-facing capabilities should use `async fn`.
3. Domain methods must not spawn background tasks implicitly.
4. Runtime-created services must return explicit handles with shutdown semantics.
5. Long-lived background behavior must have a clearly defined owner.

Examples of explicit task-owned products:

- `ProxyHandle`
- `KeepaliveHandle`
- internal runtime supervisor handle

The redesign must remove patterns where resources are kept alive through detached tasks without explicit ownership.

## Function Parameter Policy

The current code frequently passes bundles such as:

- server address
- derived token
- client IP
- legacy cipher hint

These should be replaced by named typed context objects when they travel together.

Preferred examples:

- `TunnelBootstrap`
- `AuthenticatedSessionSeed`
- `KeepalivePolicy`
- `ControlPlaneCredentials`

Rules:

1. If multiple parameters almost always travel together, replace them with a named struct.
2. Do not expose protocol bootstrap fragments at the facade layer.
3. Replace `Option<&str>` for protocol strategy where possible with typed enums or typed configuration values.

## Struct And Builder Policy

`EasyConnectConfig` should be replaced by a cleaner separation between:

- builder state for user intent collection
- runtime assembly inputs
- test-only support types

The intended direction is:

- `EasyConnectClientBuilder` collects end-user options
- `EasyConnectClient` is the constructed facade entrypoint
- runtime-specific assembly artifacts stay internal

Testing injection points should not be mixed into the same type that end users rely on for normal configuration.

## Error Design

Errors should be layered and typed.

Recommended direction:

- `KernelError` or finer protocol-specific errors for parsing/encoding failures
- `ControlPlaneError`
- `TunnelBootstrapError`
- `RouteDecisionError`
- `TransportError`
- `IntegrationError`
- one high-level facade error that preserves source chains

The redesign should remove stringly-typed catch-all variants such as broad `...Failed(String)` for cases where the caller or maintainer needs structured behavior.

## Module Layout

The target source layout should converge toward:

```text
src/
  facade/
    client.rs
    session.rs
    mod.rs
  domain/
    connect_target.rs
    route_policy.rs
    resolver.rs
    session_info.rs
    keepalive.rs
    mod.rs
  kernel/
    control/
      messages.rs
      parser.rs
      encoder.rs
    tunnel/
      token.rs
      handshake.rs
      parser.rs
      cipher.rs
    mod.rs
  runtime/
    control_plane/
      client.rs
      flow.rs
      types.rs
    data_plane/
      tunnel_factory.rs
      packet_pump.rs
      netstack.rs
      transport.rs
    tasks/
      keepalive.rs
      supervisor.rs
    mod.rs
  integration/
    reqwest.rs
    http_proxy.rs
    mod.rs
  error.rs
  lib.rs
```

This is the target architecture for implementation planning. Exact filenames may vary slightly if implementation reveals a cleaner split, but the layer boundaries and dependency direction are mandatory.

## Migration Strategy

Implementation planning should assume the following sequence:

1. Freeze the current codebase with `git tag v0.1`.
2. Extract pure protocol logic into `kernel`.
3. Split current `auth::control` into runtime control-plane and runtime data-plane responsibilities.
4. Introduce typed runtime assembly artifacts such as `AuthenticatedSessionSeed` and `TunnelBootstrap`.
5. Replace `EasyConnectConfig` and `EasyConnectSession` facade construction with `EasyConnectClientBuilder`, `EasyConnectClient`, and the new stable `Session`.
6. Rework `reqwest` integration and HTTP proxy to depend only on stable `Session` capabilities.
7. Rebuild error types around layered typed errors.
8. Move test harness and support code out of production module bodies where practical.
9. Update the crate version to `0.2.0` once the redesign is implemented and verified.

## Testing Requirements

Implementation planning must include tests for each architectural layer:

- `kernel`: parsers, encoders, token derivation, handshake builders
- `runtime`: flow orchestration, task ownership, tunnel setup behavior
- `domain`: route decisions, target normalization, session information behavior
- `integration`: HTTP proxy and `reqwest` behavior through stable `Session` APIs
- facade-level smoke tests for end-to-end happy paths with existing harnesses or revised test support

The redesign should also preserve existing behavior coverage where the current tests already exercise real integration assumptions.

## Acceptance Criteria

The redesign is complete when:

1. The public API is centered on `EasyConnectClient` and `Session`.
2. High-level public types no longer expose protocol bootstrap internals.
3. `auth::control` is no longer a god module with mixed responsibilities.
4. Runtime background tasks have explicit ownership and shutdown semantics.
5. Protocol logic is isolated in a pure `kernel` layer.
6. `reqwest` and proxy integration depend on stable session capabilities rather than hidden side effects.
7. Error handling is structured enough for precise tests and source preservation.
8. The implementation can be planned and executed as a coherent single redesign rather than as multiple unrelated projects.
