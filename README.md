# smelly-connect

Rust 实现的 EasyConnect VPN 客户端。

## 架构

```
┌─────────────────────────────────┐
│        smelly-connect-cli        │  ← 多账号池 / HTTP & SOCKS5 代理 / 管理 API
│    (config.toml / CLI 命令)      │
├─────────────────────────────────┤
│        smelly-connect            │  ← 控制面登录 / 资源解析 / 数据面 / TCP/UDP/ICMP
│  (Session / TransportStack /     │
│   netstack smoltcp)              │
├─────────────────────────────────┤
│          smelly-tls              │  ← TLS 1.1 最小实现 (RC4-SHA / AES-128-SHA)
│   (EasyConnect 旧式隧道)         │
└─────────────────────────────────┘
```

- **smelly-connect** — 控制面认证、资源解析、会话管理、基于 smoltcp 的用户态 TCP/IP 栈、HTTP/SOCKS5 代理库。
- **smelly-connect-cli** — CLI 工具，支持前台 `proxy` 服务、`test` 诊断、`inspect` / `status` / `routes` 查询，可选管理 API。
- **smelly-tls** — 面向 EasyConnect 旧协议的 TLS 1.1 客户端，用于数据面隧道握手与加解密。

主库版本：`smelly-connect v0.2.0`

## 快速开始

```bash
# 构建
cargo build --workspace

# 运行所有测试
cargo test --workspace

# 构建 CLI（release）
cargo build -p smelly-connect-cli --release

# 带管理 API 的构建
cargo build -p smelly-connect-cli --release --features management-api
```

### 最小配置

创建 `config.toml`：

```toml
[vpn]
server = "vpn1.sit.edu.cn"

[[accounts]]
name = "my-account"
username = "your_username"
password = "your_password"

[proxy.http]
enabled = true
listen = "127.0.0.1:8080"
```

启动代理：

```bash
smelly-connect-cli proxy
```

测试：

```bash
curl -x http://127.0.0.1:8080 https://jwxt.sit.edu.cn -I
```

## 配置参考

### `[vpn]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `server` | string | **必填** | VPN 服务器地址（不带协议前缀） |
| `enable_icmp_keepalive` | bool | `true` | ICMP keepalive 总开关 |
| `default_keepalive_host` | string | — | keepalive 默认目标（如未设置则不启动 ICMP 探测） |

### `[pool]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `prewarm` | u64 | `1` | 启动时预建联数 |
| `connect_timeout_secs` | u64 | `20` | 回退超时（被下面两个细粒度超时覆盖） |
| `session_connect_timeout_secs` | u64 | `connect_timeout_secs` | 单会话建联超时 |
| `healthcheck_interval_secs` | u64 | `60` | 健康检查间隔 |
| `selection` | string | `"round_robin"` | 节点选择策略（当前仅支持轮询） |
| `failure_threshold` | u32 | `3` | 连续失败阈值，到达后节点摘除 |
| `backoff_base_secs` | u64 | `30` | 摘除后首次恢复等待 |
| `backoff_max_secs` | u64 | `600` | 摘除后最大恢复等待 |
| `allow_request_triggered_probe` | bool | `true` | 无可用节点时，允许首个请求提前触发一次恢复探测 |

### `[[accounts]]` （数组）

| 字段 | 类型 | 说明 |
|---|---|---|
| `name` | string | 账号标签（用于日志和 metrics） |
| `username` | string | EasyConnect 用户名 |
| `password` | string | EasyConnect 密码 |

### `[proxy]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `upstream_tcp_connect_timeout_secs` | u64 | `connect_timeout_secs` | 代理到上游的 TCP 建联超时 |
| `shutdown_drain_timeout_secs` | u64 | `30` | 收到 SIGINT 后优雅关闭等待时间 |

### `[proxy.http]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `enabled` | bool | — | 是否启用 HTTP 代理 |
| `listen` | string | — | 监听地址，如 `"127.0.0.1:8080"` |

### `[proxy.socks5]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `enabled` | bool | — | 是否启用 SOCKS5 代理 |
| `listen` | string | — | 监听地址，如 `"127.0.0.1:1080"` |
| `udp_associate_idle_timeout_secs` | u64 | — | UDP ASSOCIATE 空闲超时；`0` 或未设置表示不超时 |

### `[routing]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `allow_all` | bool | `false` | 强制所有流量走 VPN（优先级最高） |
| `default_action` | string | `"direct"` | 未命中规则的默认动作：`"direct"` 直连 / `"block"` 拒绝 |

`[[routing.domain_rules]]` 和 `[[routing.ip_rules]]` 为本地追加规则，与 VPN 服务端下发规则取并集：

```toml
[[routing.domain_rules]]
domain = "*.foo.edu.cn"
port_min = 443
port_max = 443
protocol = "tcp"

[[routing.ip_rules]]
ip_min = "42.62.107.1"
ip_max = "42.62.107.254"
protocol = "all"
```

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `domain` / `ip_min` | string | — | 域名（支持 `*` 前缀通配）或起始 IP |
| `ip_max` | string | 同 `ip_min` | 结束 IP（仅 `ip_rules`；可省略表示单 IP） |
| `port_min` | u16 | `1` | 起始端口 |
| `port_max` | u16 | `65535` | 结束端口 |
| `protocol` | string | `"all"` | `"tcp"` / `"udp"` / `"all"` |

### `[management]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `enabled` | bool | `false` | 启用管理 API（需编译 `management-api` feature） |
| `listen` | string | `"127.0.0.1:9090"` | 监听地址 |

### `[logging]`

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `mode` | string | `"stdout"` | 日志输出：`"stdout"` / `"file"` / `"stdout+file"` / `"off"` |
| `level` | string | `"info"` | 日志级别：`"error"` / `"warn"` / `"info"` / `"debug"` |
| `file` | string | `"smelly-connect.log"` | 日志文件路径 |

日志文件采用追加模式写入，不做轮转。

## CLI 命令

配置优先级：CLI 显式参数 > `config.toml` > 内建默认值。

### `proxy` — 启动代理服务

```bash
smelly-connect-cli proxy
smelly-connect-cli proxy --listen-http 0.0.0.0:8080 --listen-socks5 0.0.0.0:1080
smelly-connect-cli proxy --allow-all            # 所有目标强制走 VPN
smelly-connect-cli proxy --prewarm 4            # 覆盖 pool.prewarm
smelly-connect-cli proxy --keepalive-host jwxt.sit.edu.cn
```

启动后，HTTP 代理和 SOCKS5 代理（按配置）同时监听。收到 SIGINT/SIGTERM 后进入优雅关闭，等待 `shutdown_drain_timeout_secs` 排空现有连接后退出。

### `test` — 连接诊断

```bash
smelly-connect-cli test tcp jwxt.sit.edu.cn:443    # TCP 建联测试
smelly-connect-cli test icmp 8.8.8.8               # ICMP ping
smelly-connect-cli test http https://jwxt.sit.edu.cn/  # HTTP GET（经 VPN）
smelly-connect-cli test legacy-probe               # EasyConnect 旧协议全面探测
```

`legacy-probe` 会执行一系列诊断步骤：请求 IP、打开收发隧道、同连接重握手、同 IP 第二对隧道（同/不同租约）、完整建联后的 authserver/jwxt/xg 连通性、断后恢复、token 刷新等。

### `inspect` — 路由 / 会话检查

```bash
smelly-connect-cli inspect route jwxt.sit.edu.cn 443   # 检查目标路由结果
smelly-connect-cli inspect session                      # 查看已配置 / 就绪账号数
```

### `routes` / `status` — 管理 API 查询

```bash
smelly-connect-cli routes                                # 拉取当前路由规则
smelly-connect-cli status                                # 拉取健康状态和统计
smelly-connect-cli status --management-api 127.0.0.1:9090
```

均通过管理 API 获取数据，需要目标服务已开启 management API。

## 管理 API

编译时需启用 `management-api` feature，运行时需配置 `[management].enabled = true`。

| 端点 | 说明 |
|---|---|
| `GET /healthz` | 池健康摘要（节点状态计数） |
| `GET /stats` | 连接统计、流量统计、503 计数（按 http/socks5 分开） |
| `GET /nodes` | 逐节点状态、失败次数、当前阶段 |
| `GET /routes` | 当前已加载的域名规则、IP 规则、静态 DNS |

## 连接池

池节点状态机：

```
Configured → Connecting → Ready ⇄ Suspect
                 ↑            ↓ (累计失败达阈值)
                 ├── HalfOpen ← Open
                 └── (Disabled ← 永久认证失败，不再恢复)
```

- **Ready** — 健康，才被轮询选中
- **Suspect** — 有失败记录但仍在轮询池中
- **Open** — 摘除状态，等待指数退避结束
- **HalfOpen** — 退避结束，允许一次探测恢复
- **Disabled** — 永久认证失败（如密码错误），永不自动恢复
- 当所有节点不可用时，`allow_request_triggered_probe = true` 允许首个到达的请求触发一次提前恢复探测（具备竞态保护，不会并发触发多个）
- HTTP 空 upstream 返回 `503 Service Unavailable`；SOCKS5 返回 `0x03 Network Unreachable`
- 听众 fd 耗尽（`EMFILE`/`ENFILE`）时，代理自动退避重试 `accept()`，不会崩溃

## 已验证链路

- 连接 `vpn1.sit.edu.cn`，获取 EasyConnect 分配的客户端 IP
- 访问 `https://jwxt.sit.edu.cn/`，完整 TLS 握手
- 连续业务请求 10 分钟保活
- 纯 idle 10 分钟，仅靠 ICMP keepalive，结束后再次访问成功
- 多账号池轮询，单个账号故障自动摘除

## 当前约束

- 仅支持 EasyConnect 协议
- 实现重点在 Linux
- 仅支持 IPv4
- 图形验证码由外部回调提供，不内置识别

## 部署

### Docker

```bash
docker build -t smelly-connect-cli:latest .
docker run --rm \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:1080:1080 \
  -p 127.0.0.1:9090:9090 \
  -v "$(pwd)/config.toml:/etc/smelly-connect/config.toml:ro" \
  smelly-connect-cli:latest
```

镜像入口会先以 root 读取挂载的配置文件，再复制到容器内仅 `vpn:vpn` 可读的位置，然后以 `vpn:vpn` 启动主进程。因此宿主机上的 `config.toml` 即使保持 `0600` 也可以直接挂载使用。

也支持 `docker compose`：见 [`docker-compose.yml`](docker-compose.yml)。

### systemd

1. 二进制安装到 `/usr/local/bin/smelly-connect-cli`
2. 配置文件放到 `/etc/smelly-connect/config.toml`
3. 安装 [`smelly-connect-cli.service`](smelly-connect-cli.service) 到 `/etc/systemd/system/`
4. 创建运行用户：

```bash
sudo useradd --system --home /var/lib/smelly-connect --create-home smelly-connect
sudo systemctl daemon-reload && sudo systemctl enable --now smelly-connect-cli
```

ICMP keepalive 走 smoltcp 用户态栈 + EasyConnect 隧道，不依赖宿主机 raw socket；默认 systemd service 无需额外 capabilities。

## 代码导航

- `smelly-connect/src/facade/` — 公开 API：`EasyConnectClient`、`EasyConnectClientBuilder`
- `smelly-connect/src/session.rs` — `Session`：`connect_tcp()`、`bind_udp()`、`icmp_ping()`、`reqwest_client()`
- `smelly-connect/src/kernel/` — EasyConnect 协议内核：控制面解析、隧道报文构造
- `smelly-connect/src/transport/netstack.rs` — 基于 smoltcp 的用户态 TCP/IP 栈（actor 模型）
- `smelly-connect/src/runtime/` — 控制面流程、数据面运行时、后台任务
- `smelly-connect-cli/src/pool.rs` — 多账号连接池、健康检查、故障转移

## 环境变量（示例程序专用）

`smelly-connect` 下的 examples 使用显式环境变量，不依赖 `.env` 或 `dotenv`：

- `VPN_HOST` / `VPN_URL` — VPN 服务器地址
- `VPN_USER` / `VPN_PASS` — 登录凭据
- `TARGET_URL` / `TARGET_HOST` / `TARGET_PORT` — 测试目标
- `HOLD_SECONDS` — 保活时长
- `SMOKE_TCP=1` / `SMOKE_ICMP=1` — 单项冒烟测试
- `IDLE_MODE=1` / `KEEPALIVE_ICMP_TARGET` — idle 保活测试

```bash
# 获取分配 IP
export VPN_URL=https://vpn1.sit.edu.cn
export VPN_USER=your_username VPN_PASS=your_password
cargo run -p smelly-connect --example request_ip

# 访问 jwxt
TARGET_URL=https://jwxt.sit.edu.cn/
cargo run -p smelly-connect --example fetch_jwxt

# idle 保活 10 分钟
KEEPALIVE_ICMP_TARGET=jwxt.sit.edu.cn IDLE_MODE=1 HOLD_SECONDS=600
cargo run -p smelly-connect --example fetch_jwxt
```

## 库用法

```rust
use smelly_connect::{EasyConnectClientBuilder, TargetAddr};

let client = EasyConnectClientBuilder::new("vpn1.sit.edu.cn")
    .credentials("user", "pass")
    .with_captcha_handler(|_captcha| async { Ok("code".into()) })
    .build()?;

let session = client.connect().await?;

// TCP 连接
let stream = session.connect_tcp(("jwxt.sit.edu.cn", 443)).await?;

// reqwest 客户端（自动走 VPN）
let reqwest_client = session.reqwest_client()?;

// ICMP keepalive
let handle = session.start_icmp_keepalive("jwxt.sit.edu.cn", Duration::from_secs(30)).await?;

// SOCKS5 / 本地 HTTP 代理 见 CLI proxy 模式
```
