# Yohe smelly-connect Deployment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deploy `smelly-connect-cli` onto `sit-yohe.sunnysab.cn` as a parallel `systemd`-managed HTTP proxy using one SIT account, validate it locally on the remote host, and leave existing Yohe traffic unchanged.

**Architecture:** Reuse the existing `smelly-connect-cli` binary, `config.toml`, and `systemd` service pattern already present in the repository. Build a management-enabled release binary locally, install it onto the remote host under `/usr/local/bin` plus `/etc/smelly-connect`, run it as a dedicated `smelly-connect` system user, stop the temporary `mitmdump` listener that currently occupies the preferred validation port, and validate through loopback-only ports without touching live Yohe traffic.

**Tech Stack:** Rust stable, Cargo workspace, `smelly-connect-cli`, `systemd`, `ssh`/`scp`, remote Ubuntu 24.04 (`systemd 255`, `glibc 2.39`), `curl`, `journalctl`.

---

## Planned File Structure

### Reuse As-Is

- `deploy/smelly-connect-cli.service`
- `config.toml.example`
- `smelly-connect-cli/Cargo.toml`
- `README.md`

### Create On Remote Host

- `/usr/local/bin/smelly-connect-cli`
- `/etc/smelly-connect/config.toml`
- `/etc/systemd/system/smelly-connect.service`
- `/var/lib/smelly-connect/`
- `/var/log/smelly-connect/`

### Modify On Remote Host

- none during the first deployment step beyond creating the new files above

### Do Not Modify In This Plan

- `/root/services/easyconnect/docker-compose.yml`
- `/root/services/yohe-server/config.yml`
- `/root/services/yohe-server-v2/configs/resty.yml`
- `/etc/hosts`

## Environment Notes

- Verified remote host: Ubuntu 24.04, `systemd 255`, `glibc 2.39`
- Verified remote user `smelly-connect` does not exist yet
- Verified remote port conflict: `0.0.0.0:18080` is currently occupied by `mitmdump`, and the user approved shutting it down
- Reserve loopback-only validation ports after clearing `mitmdump`:
  - HTTP proxy: `127.0.0.1:18080`
  - management API: `127.0.0.1:19090`

## Task 1: Verify Local Build Inputs And Produce The Release Binary

**Files:**
- Reuse: `smelly-connect-cli/Cargo.toml`
- Reuse: `deploy/smelly-connect-cli.service`
- Reuse: `config.toml.example`
- Build Artifact: `target/release/smelly-connect-cli`

- [ ] **Step 1: Confirm the CLI supports the management API feature**

Run: `cargo tree -p smelly-connect-cli -e features | rg 'management-api|axum|serde_json'`
Expected: output shows the optional `management-api` feature wiring for `smelly-connect-cli`

- [ ] **Step 2: Build the release binary with management enabled**

Run: `cargo build -p smelly-connect-cli --release --features management-api`
Expected: PASS and produce `target/release/smelly-connect-cli`

- [ ] **Step 3: Smoke-check the binary help output locally**

Run: `./target/release/smelly-connect-cli --help`
Expected: PASS and print top-level CLI usage

- [ ] **Step 4: Record the binary metadata for transfer validation**

Run: `file target/release/smelly-connect-cli && sha256sum target/release/smelly-connect-cli`
Expected: ELF 64-bit Linux executable plus a SHA-256 checksum

- [ ] **Step 5: Commit any repository changes only if build prerequisites had to be adjusted**

```bash
git status --short
# Expected: no repository changes for the happy path
```

## Task 2: Prepare The Remote Host Runtime Directories And Service User

**Files:**
- Create: `/var/lib/smelly-connect/`
- Create: `/var/log/smelly-connect/`
- Create: system user `smelly-connect`

- [ ] **Step 1: Stop `mitmdump` if it is still occupying the preferred validation port**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo ss -ltnp | grep ":18080" || true
  sudo pkill -f mitmdump || true
  sudo ss -ltnp | grep ":18080" || true
'
```

Expected: the first check may show `mitmdump`; the second check should show port `18080` cleared

- [ ] **Step 2: Create the service user and directories on the remote host**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo useradd --system --home-dir /var/lib/smelly-connect --create-home smelly-connect 2>/dev/null || true
  sudo install -d -o smelly-connect -g smelly-connect /var/lib/smelly-connect
  sudo install -d -o smelly-connect -g smelly-connect /var/log/smelly-connect
  sudo install -d -o root -g root /etc/smelly-connect
'
```

Expected: PASS and leave the directories present with the intended ownership

- [ ] **Step 3: Verify the created user and directory ownership**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo id smelly-connect
  sudo ls -ld /var/lib/smelly-connect /var/log/smelly-connect /etc/smelly-connect
'
```

Expected: PASS and show `smelly-connect` plus the created directories

- [ ] **Step 4: Snapshot the current EasyConnect credentials source for later config entry**

Run: `ssh sit-yohe.sunnysab.cn 'sudo sed -n "1,120p" /root/services/easyconnect/docker-compose.yml'`
Expected: PASS and show the current `easyconnect-1` account stanza to copy into the new config manually

- [ ] **Step 5: Do not commit anything for this remote-only task**

```bash
true
```

## Task 3: Install The Binary, Render The Remote Config, And Register The systemd Unit

**Files:**
- Create: `/usr/local/bin/smelly-connect-cli`
- Create: `/etc/smelly-connect/config.toml`
- Create: `/etc/systemd/system/smelly-connect.service`
- Reuse Template: `deploy/smelly-connect-cli.service`

- [ ] **Step 1: Copy the release binary to the remote host**

Run:

```bash
scp /home/sunnysab/Code/1-SIT/smelly-connect/target/release/smelly-connect-cli \
  sit-yohe.sunnysab.cn:/tmp/smelly-connect-cli
ssh sit-yohe.sunnysab.cn '
  sudo install -o root -g root -m 0755 /tmp/smelly-connect-cli /usr/local/bin/smelly-connect-cli
  rm -f /tmp/smelly-connect-cli
'
```

Expected: PASS and the binary exists at `/usr/local/bin/smelly-connect-cli`

- [ ] **Step 2: Verify the remote binary runs before wiring the service**

Run: `ssh sit-yohe.sunnysab.cn '/usr/local/bin/smelly-connect-cli --help | head -n 20'`
Expected: PASS and print CLI usage on the remote host

- [ ] **Step 3: Render `/etc/smelly-connect/config.toml` using one SIT account and the non-conflicting ports**

Write this exact content, replacing only the account credentials with the values from `/root/services/easyconnect/docker-compose.yml`:

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
username = "REPLACE_WITH_REMOTE_VALUE"
password = "REPLACE_WITH_REMOTE_VALUE"

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

Run:

```bash
ssh sit-yohe.sunnysab.cn 'sudo tee /etc/smelly-connect/config.toml >/dev/null'
```

Expected: PASS and the config file exists with root ownership

- [ ] **Step 4: Install the `systemd` unit using the repository template as the baseline**

Install this unit content:

```ini
[Unit]
Description=smelly-connect-cli proxy service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=smelly-connect
Group=smelly-connect
WorkingDirectory=/var/lib/smelly-connect
ExecStart=/usr/local/bin/smelly-connect-cli --config /etc/smelly-connect/config.toml proxy
Restart=always
RestartSec=5
NoNewPrivileges=true
AmbientCapabilities=CAP_NET_RAW
CapabilityBoundingSet=CAP_NET_RAW
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
```

Run:

```bash
ssh sit-yohe.sunnysab.cn 'sudo tee /etc/systemd/system/smelly-connect.service >/dev/null'
ssh sit-yohe.sunnysab.cn 'sudo systemctl daemon-reload'
```

Expected: PASS and `systemctl cat smelly-connect` shows the new unit

- [ ] **Step 5: Verify the installed assets**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo ls -l /usr/local/bin/smelly-connect-cli /etc/smelly-connect/config.toml /etc/systemd/system/smelly-connect.service
  sudo systemctl cat smelly-connect
'
```

Expected: PASS and show all three installed assets

## Task 4: Start The Service And Validate Management Health

**Files:**
- Read: `/etc/smelly-connect/config.toml`
- Read: `/etc/systemd/system/smelly-connect.service`
- Read: `/var/log/smelly-connect/smelly-connect.log`

- [ ] **Step 1: Enable and start the service**

Run: `ssh sit-yohe.sunnysab.cn 'sudo systemctl enable --now smelly-connect'`
Expected: PASS and the service enters `active (running)` or quickly restarts into that state

- [ ] **Step 2: Inspect the first boot logs**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo systemctl status --no-pager smelly-connect
  sudo journalctl -u smelly-connect -n 100 --no-pager
'
```

Expected: PASS and logs show configuration load, session startup, and no persistent fatal crash loop

- [ ] **Step 3: Verify the management API is healthy**

Run: `ssh sit-yohe.sunnysab.cn 'curl -fsS http://127.0.0.1:19090/healthz'`
Expected: PASS and return JSON with a healthy or otherwise usable status payload

- [ ] **Step 4: Verify the loopback listener bindings**

Run: `ssh sit-yohe.sunnysab.cn 'sudo ss -ltnp | grep -E "127.0.0.1:(18080|19090)"'`
Expected: PASS and show the running `smelly-connect-cli` process listening on both loopback ports

- [ ] **Step 5: If startup fails, stop here and capture the exact error before any retry loop changes**

```bash
ssh sit-yohe.sunnysab.cn '
  sudo systemctl stop smelly-connect
  sudo journalctl -u smelly-connect -n 200 --no-pager
'
```

Expected: only use this branch on failure

## Task 5: Validate Proxy Function Through The New Endpoint Without Changing Live Traffic

**Files:**
- Read: `/var/log/smelly-connect/smelly-connect.log`
- Read: remote health endpoint `http://127.0.0.1:19090/healthz`
- Read: remote stats endpoint `http://127.0.0.1:19090/stats`

- [ ] **Step 1: Run a local HTTP request through the new proxy toward a known SIT target**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  HTTPS_PROXY=http://127.0.0.1:18080 \
  HTTP_PROXY=http://127.0.0.1:18080 \
  curl -I -k --max-time 20 https://jwxt.sit.edu.cn/
'
```

Expected: PASS and return response headers from the target or at least a successful CONNECT chain

- [ ] **Step 2: Check management stats after the probe**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  curl -fsS http://127.0.0.1:19090/healthz
  curl -fsS http://127.0.0.1:19090/stats
'
```

Expected: PASS and show at least one handled connection in the stats payload

- [ ] **Step 3: Confirm the file log captured the request path**

Run: `ssh sit-yohe.sunnysab.cn 'sudo tail -n 100 /var/log/smelly-connect/smelly-connect.log'`
Expected: PASS and show request or session activity around the test window

- [ ] **Step 4: Leave Yohe services unchanged and document the current cutover boundary**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo grep -n "proxy_addr" /root/services/yohe-server/config.yml
  sudo grep -n "proxy_addr_list" /root/services/yohe-server-v2/configs/resty.yml
'
```

Expected: PASS and confirm the old proxy paths remain configured

- [ ] **Step 5: Commit only if repository deployment assets had to be updated during implementation**

```bash
git status --short
# Expected: no repository changes for the happy path
```

## Task 6: Capture Rollback Commands And Completion Evidence

**Files:**
- Read: `/etc/systemd/system/smelly-connect.service`
- Read: `/etc/smelly-connect/config.toml`
- Read: `/var/log/smelly-connect/smelly-connect.log`

- [ ] **Step 1: Record the minimum rollback sequence**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo systemctl stop smelly-connect
  sudo systemctl disable smelly-connect
'
```

Expected: do not execute unless rollback is needed; keep this as the validated rollback path

- [ ] **Step 2: Record the optional cleanup sequence**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo rm -f /etc/systemd/system/smelly-connect.service
  sudo rm -f /etc/smelly-connect/config.toml
  sudo rm -f /usr/local/bin/smelly-connect-cli
  sudo systemctl daemon-reload
'
```

Expected: do not execute unless full removal is needed

- [ ] **Step 3: Capture final evidence for handoff**

Run:

```bash
ssh sit-yohe.sunnysab.cn '
  sudo systemctl status --no-pager smelly-connect
  curl -fsS http://127.0.0.1:19090/healthz
  sudo tail -n 50 /var/log/smelly-connect/smelly-connect.log
'
```

Expected: PASS and provide the final artifact set proving the parallel deployment is healthy

- [ ] **Step 4: Summarize what remains for the later cutover**

Run:

```bash
printf '%s\n' \
  '/root/services/yohe-server/config.yml still points at http://192.168.11.2:8080' \
  '/root/services/yohe-server-v2/configs/resty.yml still points at http://100.114.29.1:8080 and http://172.17.69.204:18080' \
  '/etc/hosts remains unchanged'
```

Expected: PASS and make the cutover boundary explicit

- [ ] **Step 5: Commit repository changes only if implementation added or updated deployment assets**

```bash
git status --short
# Expected: no repository changes for the happy path
```
