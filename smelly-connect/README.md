# smelly-connect

Rust rewrite of EasyConnect-oriented intranet access tooling.

Current status:

- library scaffolding, parsing, routing, transport abstractions
- local HTTP proxy
- reqwest helper through an internal local proxy

## Manual Verification

- password-only login against a real EasyConnect server
- captcha callback flow against a real EasyConnect server
- assigned client IP retrieval from the real protocol reply
- local HTTP proxy request to an intranet HTTP target
- local HTTPS CONNECT proxy request to an intranet HTTPS target
- `reqwest_client()` request to an intranet HTTP target
