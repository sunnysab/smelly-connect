# smelly-connect

`smelly-connect` 是一个用 Rust 重写的 EasyConnect 客户端工作区，目标是替代现有 Go 版本里的 EasyConnect 数据面与访问能力。

当前仓库包含两个 crate：

- `smelly-connect`
  EasyConnect 控制面登录、资源解析、会话管理、本地 HTTP 代理、`reqwest` 适配、Rust 数据面与保活逻辑。
- `smelly-tls`
  面向 EasyConnect 旧协议的最小 TLS 1.1 客户端实现，用于旧式隧道握手与数据传输。

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

## 环境变量

示例程序默认读取这些环境变量：

- `VPN_HOST` 或 `VPN_URL`
- `VPN_USER` 或 `USER`
- `VPN_PASS` 或 `PASS`

可选变量：

- `TARGET_URL`
- `HOLD_SECONDS`
- `SMOKE_TCP=1`
- `SMOKE_ICMP=1`
- `IDLE_MODE=1`
- `KEEPALIVE_ICMP_TARGET`

一个典型的 `.env` 可以是：

```env
USER=your_username
PASS=your_password
VPN_URL=https://vpn1.sit.edu.cn
```

## 示例

获取服务端分配 IP：

```bash
set -a
source .env
set +a
VPN_HOST=${VPN_URL#https://}
VPN_HOST=${VPN_HOST#http://}
VPN_USER=$USER
VPN_PASS=$PASS
cargo run -p smelly-connect --example request_ip
```

调试目标路由是否命中资源规则：

```bash
set -a
source .env
set +a
VPN_URL=${VPN_URL:-https://vpn1.sit.edu.cn}
TARGET_HOST=jwxt.sit.edu.cn
cargo run -p smelly-connect --example debug_route
```

访问 `jwxt.sit.edu.cn`：

```bash
set -a
source .env
set +a
VPN_URL=${VPN_URL:-https://vpn1.sit.edu.cn}
TARGET_URL=https://jwxt.sit.edu.cn/
cargo run -p smelly-connect --example fetch_jwxt
```

只测试 TCP 建链：

```bash
set -a
source .env
set +a
VPN_URL=${VPN_URL:-https://vpn1.sit.edu.cn}
TARGET_URL=https://jwxt.sit.edu.cn/
SMOKE_TCP=1
cargo run -p smelly-connect --example fetch_jwxt
```

只测试 ICMP Echo：

```bash
set -a
source .env
set +a
VPN_URL=${VPN_URL:-https://vpn1.sit.edu.cn}
TARGET_URL=https://jwxt.sit.edu.cn/
SMOKE_ICMP=1
cargo run -p smelly-connect --example fetch_jwxt
```

纯 idle 保活 10 分钟，然后再访问一次页面：

```bash
set -a
source .env
set +a
VPN_URL=${VPN_URL:-https://vpn1.sit.edu.cn}
TARGET_URL=https://jwxt.sit.edu.cn/
KEEPALIVE_ICMP_TARGET=jwxt.sit.edu.cn
IDLE_MODE=1
HOLD_SECONDS=600
cargo run -p smelly-connect --example fetch_jwxt
```

## 代码入口

比较关键的模块：

- `smelly-connect/src/config.rs`
  会话配置与默认 bootstrap。
- `smelly-connect/src/auth/control.rs`
  EasyConnect 控制面、token、IP 获取、legacy tunnel 建立。
- `smelly-connect/src/session.rs`
  会话对象、路由决策、HTTP 代理入口、ICMP keepalive。
- `smelly-connect/src/transport/netstack.rs`
  基于 `smoltcp` 的 Rust 数据面。
- `smelly-tls/src/lib.rs`
  EasyConnect 旧 TLS 路径所需的最小实现。

## 对外能力

当前可直接使用的能力包括：

- 创建 `EasyConnectConfig`
- 提供验证码回调
- 建立 `EasyConnectSession`
- 调用 `connect_tcp()`
- 启动本地 HTTP 代理
- 获取 `reqwest_client()`
- 启动后台 ICMP keepalive

## 说明

- 这里的 keepalive 不是 EasyConnect 专有隧道 heartbeat loop，而是更接近 Go 参考实现里的“周期性活跃流量保活”。
- 当前 `smelly-tls` 的 `ClientHello` 已经带有 heartbeat 扩展宣告。
- `autoresearch-state.json`、`research-results.tsv`、`autoresearch-lessons.md` 一类运行工件不会提交到仓库。
