# EasyConnect Rust Rewrite Design

- Date: 2026-03-25
- Status: Draft approved in conversation, pending written-spec review
- Scope: First version of a Rust rewrite based on `/home/sunnysab/RE/easyconnect/zju-connect`

## Goal

Build a Rust implementation of the EasyConnect client flow for Linux that supports:

- Password login
- Image captcha challenge via external callback
- Fetching the assigned VPN client IP
- Exposing a local HTTP/HTTPS CONNECT proxy for accessing intranet resources
- Exposing Rust APIs for creating reusable async streams and higher-level HTTP client integration

This version only needs to support the EasyConnect protocol. It does not need to support aTrust, TUN mode, UDP forwarding, captcha recognition, or automatic re-login.

## Non-Goals

The first version will not include:

- aTrust protocol support
- SOCKS proxy
- TUN mode or system-wide routing
- UDP support
- Automatic captcha recognition
- Automatic re-login after session expiry
- Windows, macOS, iOS, or Android platform support
- Certificate auth, SMS auth, or TOTP auth

## Constraints And Assumptions

- Primary target runtime is Linux.
- Public APIs should be Tokio-async first.
- The implementation should preserve the EasyConnect wire behavior needed for:
  - login_auth
  - optional rand_code retrieval
  - login_psw
  - token derivation
  - IP retrieval
  - optional config and resource retrieval
- Domain access is a first-class requirement. The client must support both IP targets and domain targets.
- The project should produce both:
  - a Rust library for embedding
  - a CLI daemon that starts the local HTTP proxy

## Recommended Architecture

Use a single Cargo package that contains both:

- `lib.rs` for the embeddable API
- `main.rs` for the CLI daemon

Internal modules should be organized by responsibility rather than by binary-vs-library split.

### Module Layout

- `auth`
  - Password login flow
  - Captcha challenge handling
  - Parsing TwfID, RSA key, exponent, CSRF, and login result
- `protocol`
  - Special TLS handshake behavior used by EasyConnect
  - Token derivation
  - Assigned-IP request and response parsing
  - Send/receive channel handshakes for the VPN transport
- `session`
  - Long-lived authenticated state
  - Shared routing, DNS, and transport state
  - Public session methods used by proxy and embedding callers
- `resource`
  - Parsing of server config and resource payloads
  - Representation of IP resources, domain resources, and DNS rules
- `resolver`
  - Session-aware domain resolution pipeline
  - Static DNS mapping, remote DNS, and system DNS fallback
- `transport`
  - Async stream creation over the EasyConnect session
  - Reconnection of low-level send/receive channels when appropriate
- `proxy::http`
  - Local HTTP proxy
  - HTTPS CONNECT tunneling
- `integration::reqwest`
  - Helper APIs for using the session with Hyper/Reqwest
- `error`
  - Structured public and internal error types

## Core Public Types

### `EasyConnectConfig`

Builder-style configuration for:

- server address and port
- username and password
- timeouts
- whether to fetch config/resources
- whether non-resource targets should be rejected or allowed to fall back to direct mode in the future
- captcha callback

### `EasyConnectSession`

Represents one authenticated EasyConnect session and acts as the single source of truth for:

- `twfid`
- derived `token`
- assigned client IP
- parsed IP and domain resources
- DNS data and resolution policy
- transport/channel manager

Public methods should include the equivalent of:

```rust
impl EasyConnectSession {
    pub fn client_ip(&self) -> std::net::Ipv4Addr;
    pub async fn connect_tcp<T: IntoTargetAddr>(&self, target: T) -> Result<VpnStream>;
    pub async fn start_http_proxy(&self, bind: std::net::SocketAddr) -> Result<HttpProxyHandle>;
    pub fn reqwest_connector(&self) -> Result<ReqwestConnector>;
    pub fn reqwest_client(&self) -> Result<reqwest::Client>;
}
```

### `VpnStream`

The returned connection type for embedded callers.

It should implement:

- `tokio::io::AsyncRead`
- `tokio::io::AsyncWrite`

If required by the selected Hyper version, it should also provide a thin compatibility wrapper for Hyper runtime IO traits.

The design intentionally does not expose a raw file descriptor contract. The EasyConnect-backed transport is not a normal kernel TCP socket, so `AsyncRead + AsyncWrite` is the stable abstraction.

### Captcha Callback

The config should accept an async callback that receives image bytes and returns the text entered by the external caller:

```rust
async fn on_captcha(image: Vec<u8>, mime_type: Option<String>) -> Result<String, CaptchaError>
```

This callback is only invoked when the server requires an image captcha.

## Session Lifecycle

The first-version login and setup flow should be:

1. Create config with credentials and captcha callback.
2. `connect().await` starts the EasyConnect setup flow.
3. Request `https://<server>/por/login_auth.csp?apiversion=1`.
4. Parse:
   - `TwfID`
   - `RSA_ENCRYPT_KEY`
   - `RSA_ENCRYPT_EXP`
   - optional `CSRF_RAND_CODE`
   - `RndImg`
5. If captcha is required:
   - request `https://<server>/por/rand_code.csp?apiversion=1`
   - pass image bytes to the external async callback
   - receive the captcha text from the callback
6. Encrypt the password according to EasyConnect behavior and submit `login_psw`.
7. Validate the success response and updated `TwfID`.
8. Perform the special TLS exchange needed to derive the EasyConnect token.
9. Request the assigned VPN client IP.
10. Optionally fetch:
   - `/por/conf.csp`
   - `/por/rclist.csp`
11. Parse and store:
   - IP resources
   - domain resources
   - DNS mapping rules
   - remote DNS server when present
12. Return an `EasyConnectSession`.

All later operations should reuse this session rather than duplicating login logic.

## Routing And DNS Strategy

The implementation should centralize all routing decisions inside the session.

### Target Types

The session must support both:

- IP targets such as `10.1.2.3:443`
- Domain targets such as `libdb.zju.edu.cn:443`

### Resolution Pipeline

For domain targets, the resolver pipeline should be:

1. Check parsed domain resources to determine whether the domain is a VPN candidate.
2. Check static DNS rules from server resources.
3. If needed, query the server-provided remote DNS server.
4. If remote DNS is unavailable or fails, fall back to system DNS.
5. Re-check the resolved IP against IP resource rules before dialing.

### Routing Policy

Default behavior for the first version:

- Targets that match VPN resources are dialed through the EasyConnect transport.
- Targets that do not match VPN resources are rejected.

The spec deliberately chooses reject-by-default for non-resource targets in v1. This keeps behavior predictable and avoids accidental direct-network leakage. A direct-fallback mode can be added later as a separate feature.

The same routing path must be reused by:

- `connect_tcp`
- the local HTTP proxy
- Reqwest/Hyper integration helpers

## Transport Design

The transport layer is session-backed and async-first.

### Requirements

- It must create an application-facing async byte stream for a target host and port.
- It must hide the low-level EasyConnect send/receive channel management.
- It must support reconnecting low-level channels when recoverable transport errors occur.
- It must not silently re-run interactive authentication flows.

### Reconnection Policy

When a low-level send or receive channel breaks:

- first try to rebuild the affected low-level transport channels
- preserve the higher-level session if the token is still valid
- surface an error if the session appears expired or unusable

This version should not perform automatic full re-login. If auth state expires, the caller must create a new session.

## Local HTTP Proxy

The CLI daemon and embedded API should both expose the same proxy implementation.

### Required Capabilities

- Listen on a caller-provided local bind address
- Forward plain HTTP requests
- Support HTTPS `CONNECT`
- Reuse the session routing and DNS policy
- Return `502 Bad Gateway` on upstream dial or forwarding failures
- Support graceful shutdown through a returned handle

### Embedded API

`start_http_proxy()` should return a handle that allows:

- observing the bind address
- waiting on the server task if needed
- graceful shutdown

## Reqwest And Hyper Integration

The library should provide a higher-level integration layer in addition to raw streams.

### Design Target

Provide one of:

- a Hyper connector built on top of `EasyConnectSession`
- a Reqwest helper that constructs a client using that connector

The stable requirement is that embedded callers must be able to:

- obtain a reusable `VpnStream`
- or create a ready-to-use HTTP client path without going through the local proxy

If Reqwest version constraints require a thin compatibility layer around Hyper, that is acceptable as long as the public session API remains stable.

## Error Model

Use structured errors instead of string-only failures.

Minimum categories:

- `AuthError`
  - invalid credentials
  - captcha required
  - captcha rejected
  - malformed login response
- `ProtocolError`
  - TLS handshake mismatch
  - token derivation failure
  - assigned-IP request failure
  - malformed protocol reply
- `ResolveError`
  - remote DNS failure
  - system DNS failure
  - no usable record found
- `RouteError`
  - target not allowed by server resources
  - unsupported target form
- `ProxyError`
  - listener bind failure
  - upstream forwarding failure
- `TransportError`
  - channel establishment failure
  - stream read/write failure
  - session expired

## Testing Strategy

### Unit Tests

Add focused tests for:

- parsing login-auth responses
- parsing resource payloads
- password encryption inputs and outputs
- route decision logic
- DNS fallback order

### Protocol Sample Tests

Add sample-driven tests using captured or redacted fixtures for:

- login-auth response
- captcha retrieval response
- login-psw success/failure response
- config response
- resource response
- assigned-IP response

### Manual Verification

Before claiming the implementation complete, verify against a real EasyConnect environment:

- password-only login
- login requiring image captcha callback
- assigned client IP retrieval
- HTTP proxy access to intranet HTTP targets
- HTTPS CONNECT proxy access to intranet HTTPS targets
- embedded client access through the library integration path

## Initial Delivery Plan Boundary

The first implementation plan should cover only:

- project scaffolding
- login flow for password auth
- captcha callback support
- token derivation
- client IP retrieval
- resource and DNS parsing
- async session object
- TCP connect API
- local HTTP proxy
- Reqwest/Hyper integration
- tests for parsing and routing logic

Anything beyond that should be treated as a later milestone.

## Open Decisions Resolved In This Spec

- Only EasyConnect is in scope for v1.
- Linux is the only target platform for v1.
- Tokio async is the primary runtime model.
- The project ships as both library and daemon.
- Domain access is required in v1.
- Captcha handling is done by an external callback that returns text.
- The stable transport abstraction is an async stream, not a raw file descriptor.
- Non-resource targets are rejected by default in v1.
