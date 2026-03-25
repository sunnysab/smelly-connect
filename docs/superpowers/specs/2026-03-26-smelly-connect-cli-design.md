# smelly-connect-cli Design

- Date: 2026-03-26
- Status: Reviewed draft
- Scope: Add a separate workspace CLI crate that consumes `smelly-connect` as a library and provides connection, testing, and proxy-serving workflows
- Workspace Direction: `smelly-connect` remains the library crate, `smelly-connect-cli` becomes the user-facing application crate

## Goal

Add a dedicated CLI application crate for operating `smelly-connect` in real environments without turning the library crate into an application shell.

The CLI must support:

- a standalone workspace member named `smelly-connect-cli`
- a single binary with multiple subcommands
- loading configuration from `config.toml`
- HTTP proxy serving
- SOCKS5 proxy serving
- testing commands for TCP, ICMP, and HTTP behavior
- inspection commands for route matching and pool/session state
- a connection pool backed by multiple accounts
- round-robin assignment of new outbound connections across ready accounts
- mixed pool startup behavior: prewarm some accounts, lazily connect the rest
- foreground-only long-running service mode, intended for `systemd`, `nohup`, or equivalent supervision

## Non-Goals

This design does not include:

- built-in daemon lifecycle management such as `start`, `stop`, or `status`
- PID files, socket activation, or service supervision internals
- GUI workflows
- changing `smelly-connect` into an application crate
- moving CLI-only orchestration into the library public API
- advanced per-target routing policy inside the pool beyond round-robin selection

## Current Constraints

The current workspace has:

- `smelly-connect` as the main library crate
- `smelly-tls` as protocol support
- examples for ad hoc connection and inspection workflows
- no real CLI beyond a stub `main.rs`

The library already has enough primitives to support an application layer:

- `EasyConnectClient` and builder-oriented façade
- `Session`
- `reqwest_client()`
- HTTP proxy support
- keepalive handles

The missing layer is operational orchestration: configuration loading, multi-account pooling, CLI command design, and SOCKS5 serving.

## Design Principles

1. `smelly-connect` stays a reusable library crate.
2. `smelly-connect-cli` owns all application workflow and long-running orchestration.
3. The CLI must consume stable library-facing APIs, not internal `kernel`, `runtime`, or `auth::control` surfaces.
4. Pooling is an application concern, not a library primitive in this iteration.
5. Long-running proxy service remains foreground-oriented and externally supervised.
6. New CLI features must not push application concepts back into the library façade unless they are truly reusable library abstractions.

## Workspace Layout

The workspace should become:

```text
.
├── Cargo.toml
├── smelly-connect/
├── smelly-connect-cli/
└── smelly-tls/
```

`Cargo.toml` at the workspace root should add `smelly-connect-cli` as a member.

`smelly-connect-cli` should depend on:

- `smelly-connect`
- `tokio`
- a CLI parser crate
- a TOML config parser stack
- logging/tracing crates
- a SOCKS5 protocol implementation or a focused in-crate implementation if the chosen library is not a good fit

## High-Level Layering

The intended dependency direction is:

```text
smelly-connect-cli -> smelly-connect
smelly-connect-cli -> tokio / cli / config / logging dependencies

smelly-connect -/-> smelly-connect-cli
```

The CLI crate must not require internal visibility into `smelly-connect` internals.

## CLI Crate Structure

Recommended `smelly-connect-cli` layout:

```text
smelly-connect-cli/
  src/
    main.rs
    cli.rs
    config.rs
    pool.rs
    commands/
      mod.rs
      proxy.rs
      test.rs
      inspect.rs
    proxy/
      mod.rs
      http.rs
      socks5.rs
```

Responsibilities:

- `main.rs`
  Initializes logging, loads CLI arguments, dispatches the chosen command.
- `cli.rs`
  Defines CLI arguments, subcommands, and option parsing.
- `config.rs`
  Loads and validates `config.toml`.
- `pool.rs`
  Owns account pool state, prewarm policy, lazy connection, health tracking, and round-robin selection.
- `commands/proxy.rs`
  Starts HTTP and/or SOCKS5 proxy services in the foreground.
- `commands/test.rs`
  Runs one-shot connectivity or request checks.
- `commands/inspect.rs`
  Prints route and pool/session state summaries.
- `proxy/http.rs`
  Accepts local HTTP/CONNECT traffic and forwards via pooled sessions.
- `proxy/socks5.rs`
  Accepts local SOCKS5 traffic and forwards via pooled sessions.

## Command Surface

The CLI should be a single binary with stable subcommands:

- `proxy`
- `test tcp`
- `test icmp`
- `test http`
- `inspect route`
- `inspect session`

### `proxy`

Primary long-running command.

Responsibilities:

- load config
- initialize the account pool
- prewarm configured accounts
- start HTTP proxy if enabled
- start SOCKS5 proxy if enabled
- continue serving in the foreground as long as at least one account is usable

Suggested options:

- `--config <path>`
- `--listen-http <addr>`
- `--listen-socks5 <addr>`
- `--prewarm <n>`
- `--keepalive-host <host>`
- `--log-format <text|json>` if logging format is introduced

### `test tcp`

One-shot command for establishing a target TCP connection through the library and pool selection logic.

### `test icmp`

One-shot command for validating the library’s session-level ICMP keepalive path through a selected session.

This command is explicitly defined as:

- selecting one ready session from the pool
- calling the library’s session-level ICMP capability against a target host or IPv4 address
- reporting success or failure as an application-level connectivity check

It is not a requirement to implement raw system ICMP tooling outside the library’s current capability model.

### `test http`

One-shot command for fetching a URL through a selected session, primarily to validate real application traffic.

### `inspect route`

Shows how the library would treat a target host/port using the current session’s resource rules and resolver data.

For this command, “allowed” means:

- host and port match the resource-derived allowlist already enforced by the library
- if the input is a hostname, it is evaluated against current domain rules and, if applicable, resolver output
- if the input is an IP, it is evaluated against current IP rules

This command must reflect the library’s existing route decision semantics rather than introduce a new CLI-only routing policy model.

### `inspect session`

Shows pool/session summary including readiness counts and recent failures.

## Configuration File

The CLI should load a TOML file whose name is explicitly supported as `config.toml`.

Recommended layout:

```toml
[vpn]
server = "vpn1.sit.edu.cn"
default_keepalive_host = "jwxt.sit.edu.cn"

[pool]
prewarm = 2
connect_timeout_secs = 20
healthcheck_interval_secs = 60
selection = "round_robin"

[[accounts]]
name = "acct-01"
username = "user1"
password = "pass1"

[[accounts]]
name = "acct-02"
username = "user2"
password = "pass2"

[proxy.http]
enabled = true
listen = "127.0.0.1:8080"

[proxy.socks5]
enabled = true
listen = "127.0.0.1:1080"
```

The CLI should allow `--config <path>` to override the default file path.

Config path resolution:

- if `--config <path>` is provided, use that path
- otherwise default to `./config.toml` in the current working directory

CLI flag precedence:

- explicit CLI flags override `config.toml`
- `config.toml` overrides built-in defaults

## Pool Model

The pool is an application-layer service that manages multiple accounts and their sessions.

### Pool Node States

Each account node should move through states such as:

- `Configured`
- `Connecting`
- `Ready`
- `Failed`

`Ready` should hold a live `Session`.

`Failed` should retain enough recent error context and retry timing to support observability and backoff.

### Startup Policy

The startup policy is:

- prewarm the first `N` accounts, where `N = pool.prewarm`
- leave the remaining accounts configured but not connected until demand requires them
- continue startup if at least one account becomes ready
- log failed prewarm attempts instead of failing the entire process as long as some account is still usable

### Selection Policy

Selection policy is fixed to round-robin for this design:

- each new outbound connection chooses the next ready session in round-robin order
- selection happens per new outbound connection, not per request fragment
- a session that is not ready must not be eligible for selection

This is intentionally simple and should not silently evolve into sticky routing or weighted routing during implementation.

### Failure Behavior

If a selected session fails during use:

- log the failure with account identity
- mark the node unhealthy or failed
- remove it from ready rotation
- allow later retry after backoff

If the pool has no ready sessions:

- the individual command or proxy request should fail fast with a clear error
- the process should not hang indefinitely waiting for a session

If the long-running `proxy` process temporarily reaches zero ready sessions:

- it must stay alive
- it must keep listeners bound
- incoming requests must fail fast with a clear “no ready session” style error
- background reconnect / backoff logic may continue attempting to restore pool capacity

The `proxy` process should only exit for fatal startup failure, explicit signal handling, or unrecoverable runtime errors unrelated to ordinary account churn.

## Proxy Serving Model

The CLI must support both HTTP and SOCKS5 proxy servers.

### HTTP Proxy

The CLI HTTP proxy layer should use pooled sessions and reuse the library’s HTTP/TCP capabilities.

### SOCKS5 Proxy

The CLI SOCKS5 layer should support at least TCP CONNECT behavior in the first version.

This design does not require UDP ASSOCIATE in the first iteration unless a chosen implementation makes it unavoidable to explicitly reject it.

### Foreground Service

Proxy services should run in the foreground only.

That means:

- no daemonization
- no PID file management
- no built-in `start/status/stop`
- intended deployment is via an external service manager

## Library Boundary Rules

The CLI crate may use:

- `EasyConnectClient`
- `EasyConnectClientBuilder`
- `Session`
- `ProxyHandle`
- `KeepaliveHandle`
- `ConnectTarget`
- other stable façade types exported by the library

The CLI crate must not rely on:

- `kernel::*`
- `runtime::*`
- `auth::control::*`
- hidden or `pub(crate)` session internals

If the CLI implementation finds the current library façade insufficient, the fix should be to extend the library with stable reusable API, not to punch through layering.

## Logging And Observability

The CLI should log:

- configuration path in use
- number of accounts configured
- number of accounts successfully prewarmed
- each proxy listener address
- account-level failures and reconnect attempts
- pool summary when starting

The CLI should not log plaintext passwords.

Health checking in this design means:

- optional session-level ICMP keepalive when a keepalive host is configured
- otherwise a lightweight readiness check based on whether a session is currently established and not marked failed

Backoff strategy should be fixed-delay in the first implementation, driven by configuration such as `healthcheck_interval_secs`, rather than introducing exponential retry logic.

Output format:

- first implementation is human-readable text only
- JSON or machine-stable output is out of scope unless added later as an explicit feature

## Testing Requirements

Implementation planning must include tests for:

- config parsing and validation
- pool prewarm behavior
- round-robin selection across ready sessions
- startup continuation when some accounts fail
- HTTP proxy forwarding through the pool
- SOCKS5 TCP CONNECT forwarding through the pool
- CLI argument parsing
- command-level smoke tests

## Acceptance Criteria

This design is implemented successfully when:

1. The workspace has a separate `smelly-connect-cli` crate.
2. `smelly-connect` remains the library crate and does not absorb CLI orchestration concerns.
3. The CLI supports `proxy`, `test`, and `inspect` subcommands.
4. The CLI loads `config.toml` and allows overriding it via command-line flag.
5. The CLI supports both HTTP proxy and SOCKS5 proxy serving.
6. The account pool supports mixed startup: prewarm some, lazily connect the rest.
7. New outbound connections are assigned using round-robin across ready sessions.
8. Startup continues if at least one account is usable.
9. The service runs in the foreground and is intended for external supervision.
