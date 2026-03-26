# Yohe smelly-connect Deployment Design

- Date: 2026-03-26
- Status: Reviewed draft
- Scope: Deploy `smelly-connect-cli` onto `sit-yohe.sunnysab.cn` as a parallel proxy service for Yohe workloads, without changing live traffic during the first step

## Goal

Deploy `smelly-connect-cli` on the remote host `sit-yohe.sunnysab.cn` and prepare it to replace the current EasyConnect-based proxy path used by `yohe-server` and `yohe-server-v2`.

The first step must:

- install and run `smelly-connect-cli` under `systemd`
- use one existing SIT account only
- expose a new local HTTP proxy endpoint for validation
- keep the current EasyConnect deployment and current Yohe proxy settings unchanged
- provide a clear validation and rollback path before any traffic cutover

The first step must not:

- modify `yohe-server` or `yohe-server-v2` proxy targets yet
- remove the old EasyConnect docker-compose deployment
- change `/etc/hosts`
- introduce multi-account pooling during initial bring-up

## Current Remote State

Observed on `sit-yohe.sunnysab.cn`:

- Existing EasyConnect stack is managed from `/root/services/easyconnect/docker-compose.yml`
- Existing Yohe services are managed from `/root/services/yohe-server` and `/root/services/yohe-server-v2`
- `yohe-server` contains direct proxy references to `http://192.168.11.2:8080`
- `yohe-server-v2` contains rule-based proxy references including:
  - `http://100.114.29.1:8080` for `jwxt.sit.edu.cn`, `sc.sit.edu.cn`, `card.sit.edu.cn`, `xg.sit.edu.cn`, `yjsgl.sit.edu.cn`, and `210.35.66.106`
  - `http://172.17.69.204:18080` for `authserver.sit.edu.cn`
- `/etc/hosts` contains:
  - `100.114.29.1 jwxt.sit.edu.cn`
  - `100.114.29.1 xg.sit.edu.cn`

This means the remote host currently has at least two proxy entry paths in active configuration, plus hostname overrides for part of the school traffic. The first deployment step should not disturb any of them.

## Chosen Approach

Recommended approach: deploy `smelly-connect-cli` in parallel under `systemd`, validate it independently, and defer traffic cutover to a later step.

Why this approach:

- it preserves the current production path while the new proxy is verified
- it minimizes rollback to a simple `systemctl stop/disable`
- it avoids mixing bring-up issues with live-traffic behavior changes
- it allows validating login, keepalive, and proxy behavior before touching Yohe configuration

Alternatives that are intentionally rejected for the first step:

- direct in-place replacement of existing proxy targets
  - too risky because any bring-up issue becomes an immediate runtime regression
- compatibility forwarding from old proxy addresses into `smelly-connect`
  - reduces config changes but adds another network hop and makes future debugging less clear

## Target Deployment Layout

Remote layout:

- binary: `/usr/local/bin/smelly-connect-cli`
- config directory: `/etc/smelly-connect`
- config file: `/etc/smelly-connect/config.toml`
- state / working directory: `/var/lib/smelly-connect`
- log file directory: `/var/log/smelly-connect`
- service unit: `/etc/systemd/system/smelly-connect.service`

Service model:

- process manager: `systemd`
- service name: `smelly-connect.service`
- process type: long-running foreground service
- restart policy: `Restart=always`
- no extra Linux capability required for the current ICMP keepalive path because it runs in user space over the EasyConnect tunnel

The service will not replace or wrap Docker. It will run directly on the host as a normal `systemd`-managed process.

## Initial Runtime Configuration

The first deployment uses one existing SIT account only. No multi-account pool behavior is required for the first validation pass.

Configuration shape:

```toml
[vpn]
server = "vpn1.sit.edu.cn"
default_keepalive_host = "jwxt.sit.edu.cn"

[pool]
prewarm = 1
connect_timeout_secs = 20
healthcheck_interval_secs = 60
selection = "round_robin"
failure_threshold = 3
backoff_base_secs = 30
backoff_max_secs = 600
allow_request_triggered_probe = true

[[accounts]]
name = "sit-primary"
username = "REDACTED"
password = "REDACTED"

[proxy.http]
enabled = true
listen = "127.0.0.1:18080"

[proxy.socks5]
enabled = false
listen = "127.0.0.1:1080"

[management]
enabled = true
listen = "127.0.0.1:19090"

[logging]
mode = "stdout+file"
level = "info"
file = "/var/log/smelly-connect/smelly-connect.log"
```

Key decisions:

- HTTP proxy is the only required protocol for the initial deployment
- SOCKS5 stays disabled unless later verification shows a real need
- management API is enabled on loopback only for health and stats checks
- the service listens on new loopback ports so it does not conflict with existing proxy endpoints

## Build and Installation Strategy

Preferred build strategy:

1. build `smelly-connect-cli` locally from the current repository revision
2. copy the release binary to the remote host
3. install config and service unit on the remote host
4. create required service user and directories
5. start and validate the service

This avoids installing the full Rust toolchain on the remote host unless the copied binary fails to run there because of system compatibility.

Fallback strategy:

- if the copied binary cannot run on the remote host, build on the remote host with the Rust toolchain and use the same runtime layout

## Validation Plan

Validation must happen before any Yohe config change.

Step 1: service health

- `systemctl status smelly-connect`
- `journalctl -u smelly-connect`
- `curl http://127.0.0.1:19090/healthz`
- confirm login succeeds and keepalive starts

Step 2: proxy function on the remote host

- send one or more HTTP requests through `http://127.0.0.1:18080`
- verify access to school targets such as `jwxt.sit.edu.cn`
- if useful, run `smelly-connect-cli test http` or `inspect route` against known targets before or after service start

Step 3: stability check

- confirm the service remains healthy after a short idle period
- confirm log output does not show reconnect loops or persistent upstream failures

Only after all three steps pass should the deployment proceed to a separate cutover change.

## Cutover Boundary

This design intentionally stops before changing live traffic.

The later cutover step, which is out of scope for this first deployment, will decide how to update:

- `/root/services/yohe-server/config.yml`
- `/root/services/yohe-server-v2/configs/resty.yml`
- possibly `/etc/hosts`

That later step must choose whether `smelly-connect` replaces:

- only the `100.114.29.1:8080`-backed path
- both current proxy paths
- or all school-target proxy traffic in Yohe configs

## Error Handling

Expected failure classes and handling:

- login failure
  - inspect credentials, captcha requirements, and server reachability
  - keep the old EasyConnect path untouched
- service boot failure
  - inspect `journalctl`, config path, directory permissions, and binary execution permissions
- proxy request failure
  - validate management health, then validate direct CLI test commands, then inspect target-specific route behavior
- keepalive-related permission failure
  - verify the deployed service still matches the current capability model and has not been wrapped by a raw-socket-based keepalive helper

In all cases, the rollback for the first step is operationally trivial because no live consumer is switched to the new service yet.

## Rollback Plan

Rollback for the first deployment step:

1. `systemctl stop smelly-connect`
2. `systemctl disable smelly-connect` if the service should not restart
3. optionally remove the unit file, config, binary, and log directory

Because no Yohe config is changed in this step, rollback does not require restoring application configuration or restarting Yohe services.

## Testing Scope

Required verification before calling the first step complete:

- binary runs on the remote host
- service starts under `systemd`
- management API reports healthy
- local HTTP proxy requests succeed to at least one expected SIT target
- logs show stable behavior for a short observation window

Nice-to-have verification:

- `inspect route` output for at least one known school target
- one idle-and-retry observation to confirm keepalive behavior matches expectation

## Open Items For The Later Cutover Step

- whether `authserver.sit.edu.cn` should also move to `smelly-connect`
- whether `/etc/hosts` overrides remain necessary after cutover
- whether Yohe should continue using proxy-by-domain rules or move more traffic through `smelly-connect`
- whether the deployment should later expand from one account to multiple accounts
