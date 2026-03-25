# smelly-connect-cli Logging Design

- Date: 2026-03-26
- Status: Reviewed draft
- Scope: Add configurable CLI-side logging for `smelly-connect-cli`, including stdout/file/off modes and structured severity usage
- Applies To: `smelly-connect-cli` only

## Goal

Add operational logging to `smelly-connect-cli` without pushing application-specific logging concerns into the `smelly-connect` library crate.

The logging system must support:

- text logging only in the first iteration
- configurable output mode through `config.toml`
- `stdout`, `file`, `stdout+file`, and `off`
- severity-based logging with at least `info`, `warn`, `error`, and configurable filtering
- startup logging
- per-request logging for proxy traffic
- account pool lifecycle logging
- file logging in append mode

## Non-Goals

This design does not include:

- JSON log format
- log rotation
- daemon-specific journal integration
- library-global logging conventions for `smelly-connect`
- deeply verbose packet or byte-level tracing
- remote log shipping

## Design Principles

1. Logging remains an application concern owned by `smelly-connect-cli`.
2. The library crate should not grow CLI-specific logging configuration or sinks.
3. The first implementation should be simple and operationally useful, not overly configurable.
4. Log output must be useful for real operations: startup, requests, failures, and pool behavior.
5. Failure to open a log file should not bring down the proxy service if another output mode is still viable.

## Configuration Model

Add a new section to `config.toml`:

```toml
[logging]
mode = "stdout+file"
level = "info"
file = "smelly-connect.log"
```

### Fields

- `mode`
  One of:
  - `stdout`
  - `file`
  - `stdout+file`
  - `off`
- `level`
  One of:
  - `error`
  - `warn`
  - `info`
  - `debug`
- `file`
  Path to the log file when file output is used

## Defaults

If `[logging]` is omitted, defaults should be:

- `mode = "stdout"`
- `level = "info"`
- `file = "smelly-connect.log"`

## Ownership And Boundaries

Logging initialization must happen in `smelly-connect-cli`, ideally in or near `main.rs`, before command execution begins.

Responsibilities:

- `config.rs`
  parse and validate logging config
- `main.rs`
  initialize the tracing subscriber and selected writers
- `commands/proxy.rs`
  log startup summaries and top-level service lifecycle events
- `pool.rs`
  log account connection, failure, removal from rotation, and retry behavior
- `proxy/http.rs`
  log inbound HTTP and CONNECT requests
- `proxy/socks5.rs`
  log inbound SOCKS5 CONNECT requests
- `commands/test.rs` / `commands/inspect.rs`
  use logs sparingly and keep command result output primarily user-facing

The `smelly-connect` library should not gain config-driven logging sink selection as part of this design.

## Output Modes

### `stdout`

All logs go to terminal output.

### `file`

All logs append to the configured file.

### `stdout+file`

Logs are written to both terminal and file.

### `off`

Operational logging is disabled.

This should disable tracing output for normal service logs while still allowing command output to explicitly print direct results when appropriate.

Clarification:

- `mode = "off"` disables tracing-based operational logs
- fatal command failures may still print direct error messages to `stderr` before process exit
- those direct fatal stderr messages are not treated as normal tracing output

## File Behavior

File logging behavior:

- append mode
- create the file if missing
- no rotation
- no truncation

If file output is requested but the file cannot be opened:

- emit a warning to stderr if possible
- fall back to stdout when `mode = "stdout+file"`
- fall back to stdout when `mode = "file"` rather than aborting startup in the first implementation

## Log Format

The format should remain plain text in the first version.

Terminal log output should use `stderr`, not `stdout`, so command result output can remain clean on `stdout`.

Recommended structure:

```text
2026-03-26T14:05:12+08:00 INFO smelly_connect_cli::commands::proxy starting proxy service http=127.0.0.1:8080 socks5=127.0.0.1:1080
2026-03-26T14:05:18+08:00 INFO smelly_connect_cli::proxy::http request method=CONNECT target=libdb.zju.edu.cn:443 account=acct-01
2026-03-26T14:05:22+08:00 WARN smelly_connect_cli::pool account prewarm failed account=acct-02 error="..."
2026-03-26T14:05:25+08:00 ERROR smelly_connect_cli configuration load failed path=./config.toml error="..."
```

Minimum required fields:

- timestamp
- severity level
- module/target name
- human-readable message

Additional key-value context is encouraged but not required to be identical across implementations.

## Severity Rules

### `info`

Use for:

- selected config path
- logging mode and file path
- startup summary
- number of configured and ready accounts
- listener startup
- inbound proxy request arrival
- chosen account for each request

### `warn`

Use for:

- account prewarm failures
- account failures during service
- temporary loss of ready sessions
- request fast-fail due to no ready session
- file logging setup fallback behavior

### `error`

Use for:

- configuration load failure
- invalid logging mode or malformed logging config
- inability to bind listeners
- unrecoverable command failure causing process exit

### `debug`

This level may exist for filtering completeness, but the first implementation does not need extensive debug-only instrumentation.

## Request Logging

For each inbound request, the CLI should log at `info` level:

- protocol (`http`, `connect`, `socks5`)
- target host and port when known
- selected account name

The design does not require logging payload bodies or credentials.

## Pool Logging

The pool should log:

- prewarm start and result summary
- each account transition to ready
- each account failure with account name
- each retry attempt after fixed-delay backoff
- when a request cannot be served because there is no ready session

The CLI must not log plaintext passwords.

## Error Handling Expectations

Logging must not become a new source of fatal failures in ordinary cases.

Expected behavior:

- bad config path: command fails, log `error`
- listener bind failure: command fails, log `error`
- file sink unavailable: command continues with fallback, log `warn`
- no ready session for one request: request fails fast, process continues, log `warn`

## Recommended Dependencies

The implementation may use:

- `tracing`
- `tracing-subscriber`
- one lightweight time-formatting dependency if needed

Avoid introducing a heavy logging stack solely for this feature.

## Testing Requirements

Implementation planning must include tests for:

- logging config parsing
- default logging config behavior
- mode selection behavior
- file path fallback behavior
- startup `info` events
- request-level `info` events
- warning on no-ready-session fast-fail
- no plaintext password leakage in logged config or account summaries

## Acceptance Criteria

The logging feature is complete when:

1. `config.toml` supports a `[logging]` section.
2. The CLI supports `stdout`, `file`, `stdout+file`, and `off`.
3. Logs are text-only.
4. Startup emits `info`-level operational logs.
5. Incoming HTTP and SOCKS5 requests emit `info` logs with account selection context.
6. Pool failures and retries emit `warn` logs.
7. Fatal command failures emit `error` logs.
8. File logging uses append mode without rotation.
9. Logging failure does not unnecessarily abort otherwise recoverable startup.

For `mode = "off"`, fatal command failure messages may still be written directly to `stderr` outside the tracing pipeline.
