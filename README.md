# smelly-connect

`smelly-connect` 是一个用 Rust 重写的 EasyConnect 客户端工作区，目标是替代现有 Go 版本里的 EasyConnect 数据面与访问能力。

当前仓库包含两个 crate：

- `smelly-connect`
  EasyConnect 控制面登录、资源解析、会话管理、本地 HTTP 代理、`reqwest` 适配、Rust 数据面与保活逻辑。
- `smelly-connect-cli`
  基于 `smelly-connect` 的独立 CLI 工具，负责 `config.toml`、多账号池、测试命令以及 HTTP/SOCKS5 代理服务。
- `smelly-tls`
  面向 EasyConnect 旧协议的最小 TLS 1.1 客户端实现，用于旧式隧道握手与数据传输。

当前主库版本：`smelly-connect v0.2.0`

## 当前状态

已经完成的能力：

- EasyConnect 用户名/密码登录
- 图形验证码回调接口
- 资源列表解析
- 获取服务端分配的客户端 IP
- 基于 Rust 数据面的真实 TCP 访问
- 本地 HTTP 代理
- `reqwest_client()` 适配
- 真实内网 HTTPS 访问
- ICMP keepalive 保活

已验证的真实链路：

- 连接 `vpn1.sit.edu.cn`
- 获取 EasyConnect 分配 IP
- 访问 `https://jwxt.sit.edu.cn/`
- 连续业务请求保活 10 分钟
- 纯 idle 10 分钟，仅靠 ICMP keepalive，结束后再次访问 `jwxt.sit.edu.cn`

当前约束：

- 只支持 EasyConnect 协议
- 当前实现重点是 Linux
- 仅支持 IPv4
- 图形验证码识别不内置，由外部回调提供

## 工作区结构

```text
.
├── Cargo.toml
├── smelly-connect-cli/
├── smelly-connect/
└── smelly-tls/
```

## 快速开始

构建：

```bash
cargo build --workspace
```

测试：

```bash
cargo test --workspace
```

如果你只关心主库：

```bash
cargo test -p smelly-connect
```

如果你只关心 CLI：

```bash
cargo test -p smelly-connect-cli
```

## CLI

当前正式命令行入口是 `smelly-connect-cli`，配置文件默认读取当前工作目录下的 `config.toml`。

构建 CLI：

```bash
cargo build -p smelly-connect-cli --release
```

如果要启用管理 API：

```bash
cargo build -p smelly-connect-cli --release --features management-api
```

一个最小 `config.toml` 例子：

```toml
[vpn]
server = "vpn1.sit.edu.cn"
default_keepalive_host = "jwxt.sit.edu.cn"

[pool]
prewarm = 2
connect_timeout_secs = 20
healthcheck_interval_secs = 60
selection = "round_robin"
failure_threshold = 3
backoff_base_secs = 30
backoff_max_secs = 600
allow_request_triggered_probe = true

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

[routing]

[[routing.domain_rules]]
domain = "*.foo.edu.cn"
port_min = 443
port_max = 443
protocol = "tcp"

[[routing.ip_rules]]
ip_min = "42.62.107.1"
ip_max = "42.62.107.254"
port_min = 1
port_max = 65535
protocol = "all"

[management]
enabled = false
listen = "127.0.0.1:9090"

[logging]
mode = "stdout"
level = "info"
file = "smelly-connect.log"
```

当前 CLI 命令面：

```bash
smelly-connect-cli --config ./config.toml proxy --listen-http 127.0.0.1:8080 --listen-socks5 127.0.0.1:1080
smelly-connect-cli --config ./config.toml routes
smelly-connect-cli --config ./config.toml status
smelly-connect-cli --config ./config.toml inspect route jwxt.sit.edu.cn 443
smelly-connect-cli --config ./config.toml inspect session
smelly-connect-cli --config ./config.toml test tcp 10.0.0.8:443
smelly-connect-cli --config ./config.toml test icmp 10.0.0.8
smelly-connect-cli --config ./config.toml test http http://intranet.zju.edu.cn/health
```

配置优先级：

- CLI 显式参数覆盖 `config.toml`
- `config.toml` 覆盖内建默认值

本地补充路由：

- `[routing]` 用于追加本地域名/IP 放行规则，与服务端下发规则做并集
- `[[routing.domain_rules]]` 支持精确域名和 `*.example.com` 泛域名
- `[[routing.ip_rules]]` 支持单 IP 和 `ip_min` 到 `ip_max` 的 IPv4 / IPv6 范围
- 本地规则支持 `port_min` / `port_max` / `protocol`

当前实现说明：

- `smelly-connect` 库本身不依赖 `.env` / `dotenv`
- `smelly-connect-cli` 走 `config.toml`
- `smelly-connect` 下的 examples 仍然使用显式环境变量，便于快速手工调试

日志配置：

- `mode = "stdout"`
  只向终端输出文本日志
- `mode = "file"`
  只追加写入日志文件
- `mode = "stdout+file"`
  同时写终端和日志文件
- `mode = "off"`
  关闭 tracing 运营日志；致命错误仍可能直接输出到 `stderr`

第一版只支持文本日志，不做轮转；文件写入采用追加模式。

连接池韧性行为：

- 正常轮询只使用 `Ready` 和 `Suspect` 节点
- 连续失败达到 `failure_threshold` 后，节点进入摘除状态
- 摘除节点按指数退避回到恢复窗口，最大等待时间由 `backoff_max_secs` 控制
- 当所有 upstream 都暂时不可用时，`proxy` 模式仍保持监听
- HTTP 空 upstream 立即返回 `503 Service Unavailable`
- SOCKS5 空 upstream 返回 `network unreachable` (`0x03`)
- `allow_request_triggered_probe = true` 时，首个到来的请求允许提前触发一次恢复探测

管理 API：

- 这是一个独立监听口，默认建议只绑定 `127.0.0.1`
- 需要使用 `management-api` feature 编译
- 如果 `config.toml` 里启用了 `[management].enabled = true`，但二进制没有带 `management-api` feature，`proxy` 启动会直接失败
- `routes` 命令会从 `[management].listen` 拉取当前已加载的域名规则、IP 规则和静态 DNS 映射
- `status` 命令会从 `[management].listen` 拉取当前健康状态和统计信息，因此要求目标服务已开启 management API
- `GET /healthz` 返回池健康摘要
- `GET /stats` 返回当前连接数、累计连接数、双向流量统计，以及 pool 节点状态摘要
- `GET /nodes` 返回逐节点状态明细
- `GET /routes` 返回当前已加载的路由规则快照

最小返回示例：

```json
{
  "status": "healthy",
  "pool": {
    "total_nodes": 2,
    "selectable_nodes": 2,
    "ready_nodes": 2,
    "suspect_nodes": 0,
    "open_nodes": 0,
    "half_open_nodes": 0,
    "connecting_nodes": 0,
    "configured_nodes": 0
  }
}
```

```json
{
  "status": "healthy",
  "pool": {
    "total_nodes": 2,
    "selectable_nodes": 2,
    "ready_nodes": 2,
    "suspect_nodes": 0,
    "open_nodes": 0,
    "half_open_nodes": 0,
    "connecting_nodes": 0,
    "configured_nodes": 0
  },
  "total": {
    "current_connections": 3,
    "total_connections": 27,
    "client_to_upstream_bytes": 10240,
    "upstream_to_client_bytes": 55296
  },
  "http": {
    "current_connections": 1,
    "total_connections": 20,
    "client_to_upstream_bytes": 4096,
    "upstream_to_client_bytes": 32768
  },
  "socks5": {
    "current_connections": 2,
    "total_connections": 7,
    "client_to_upstream_bytes": 6144,
    "upstream_to_client_bytes": 22528
  }
}
```

## 部署

Docker：

```bash
docker build -f deploy/Dockerfile -t smelly-connect-cli:latest .
docker run --rm \
  --cap-add=NET_RAW \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:1080:1080 \
  -p 127.0.0.1:9090:9090 \
  -v "$(pwd)/config.toml:/etc/smelly-connect/config.toml:ro" \
  smelly-connect-cli:latest
```

如果代理或管理 API 需要被宿主机外访问，把 `config.toml` 里的监听地址从 `127.0.0.1` 改成对应的容器内绑定地址，例如 `0.0.0.0`。

也可以直接使用 [`deploy/docker-compose.yml`](deploy/docker-compose.yml)。

systemd：

- 把二进制放到 `/usr/local/bin/smelly-connect-cli`
- 把配置文件放到 `/etc/smelly-connect/config.toml`
- 把 [`deploy/smelly-connect-cli.service`](deploy/smelly-connect-cli.service) 安装到 `/etc/systemd/system/`
- 创建运行用户：`sudo useradd --system --home /var/lib/smelly-connect --create-home smelly-connect`
- 执行：`sudo systemctl daemon-reload && sudo systemctl enable --now smelly-connect-cli`

说明：

- 当前实现的 ICMP keepalive 走用户态 `smoltcp` 和 EasyConnect 隧道，不依赖宿主机 raw socket
- 默认 `systemd` service 不需要额外 Linux capabilities
- 当前 service 文件假设使用前台 `proxy` 模式，由 systemd 负责守护与重启

## 环境变量

示例程序读取这些显式环境变量：

- `VPN_HOST` 或 `VPN_URL`
- `VPN_USER`
- `VPN_PASS`

可选变量：

- `TARGET_URL`
- `TARGET_HOST`
- `TARGET_PORT`
- `HOLD_SECONDS`
- `SMOKE_TCP=1`
- `SMOKE_ICMP=1`
- `IDLE_MODE=1`
- `KEEPALIVE_ICMP_TARGET`

这些示例只依赖进程环境变量本身，不依赖 `.env` 文件或 `dotenv` 加载器。  
如果你想使用 shell 配置文件、direnv、systemd Environment、CI secrets，或者手工 `export`，都可以。

## 示例

获取服务端分配 IP：

```bash
export VPN_URL=https://vpn1.sit.edu.cn
export VPN_USER=your_username
export VPN_PASS=your_password
VPN_HOST=${VPN_URL#https://}
VPN_HOST=${VPN_HOST#http://}
cargo run -p smelly-connect --example request_ip
```

调试目标路由是否命中资源规则：

```bash
export VPN_URL=https://vpn1.sit.edu.cn
export VPN_USER=your_username
export VPN_PASS=your_password
TARGET_HOST=jwxt.sit.edu.cn
cargo run -p smelly-connect --example debug_route
```

访问 `jwxt.sit.edu.cn`：

```bash
export VPN_URL=https://vpn1.sit.edu.cn
export VPN_USER=your_username
export VPN_PASS=your_password
TARGET_URL=https://jwxt.sit.edu.cn/
cargo run -p smelly-connect --example fetch_jwxt
```

只测试 TCP 建链：

```bash
export VPN_URL=https://vpn1.sit.edu.cn
export VPN_USER=your_username
export VPN_PASS=your_password
TARGET_URL=https://jwxt.sit.edu.cn/
SMOKE_TCP=1
cargo run -p smelly-connect --example fetch_jwxt
```

只测试 ICMP Echo：

```bash
export VPN_URL=https://vpn1.sit.edu.cn
export VPN_USER=your_username
export VPN_PASS=your_password
TARGET_URL=https://jwxt.sit.edu.cn/
SMOKE_ICMP=1
cargo run -p smelly-connect --example fetch_jwxt
```

纯 idle 保活 10 分钟，然后再访问一次页面：

```bash
export VPN_URL=https://vpn1.sit.edu.cn
export VPN_USER=your_username
export VPN_PASS=your_password
TARGET_URL=https://jwxt.sit.edu.cn/
KEEPALIVE_ICMP_TARGET=jwxt.sit.edu.cn
IDLE_MODE=1
HOLD_SECONDS=600
cargo run -p smelly-connect --example fetch_jwxt
```

## 代码入口

比较关键的模块：

- `smelly-connect/src/facade/`
  对外 façade，暴露 `EasyConnectClient` 和稳定公开类型。
- `smelly-connect/src/domain/`
  稳定领域对象，包括 `Session`、`ConnectTarget`、`SessionInfo`、`KeepalivePolicy`。
- `smelly-connect/src/kernel/`
  纯 EasyConnect 协议内核，包含控制面解析与 legacy tunnel 报文构造。
- `smelly-connect/src/runtime/`
  控制面流程、数据面运行时、后台任务与 handle 生命周期。
- `smelly-tls/src/lib.rs`
  EasyConnect 旧 TLS 路径所需的最小实现。

## 对外能力

当前可直接使用的能力包括：

- 创建 `EasyConnectClientBuilder`
- 构建 `EasyConnectClient`
- 提供验证码回调
- 建立 `Session`
- 调用 `connect_tcp()`
- 启动本地 HTTP 代理
- 获取 `reqwest_client()`
- 启动并显式关闭 ICMP keepalive

## 说明

- 这里的 keepalive 不是 EasyConnect 专有隧道 heartbeat loop，而是更接近 Go 参考实现里的“周期性活跃流量保活”。
- 当前 `smelly-tls` 的 `ClientHello` 已经带有 heartbeat 扩展宣告。
- `autoresearch-state.json`、`research-results.tsv`、`autoresearch-lessons.md` 一类运行工件不会提交到仓库。
