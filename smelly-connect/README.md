# smelly-connect

`smelly-connect` 是工作区中的主 crate，负责 EasyConnect 的控制面、会话、路由、代理与 Rust 数据面接入。

当前 crate 版本：`0.2.0`

正式命令行入口由工作区中的 `smelly-connect-cli` crate 提供；本 crate 保持库定位。

当前已经支持：

- 用户名/密码登录
- 图形验证码回调
- 资源规则解析
- 获取分配 IP
- `EasyConnectClient::builder(...).credentials(...).build()`
- `connect_tcp()`
- 本地 HTTP 代理
- `reqwest_client()`
- ICMP keepalive

示例程序：

- `request_ip`
- `debug_route`
- `fetch_jwxt`

更完整的中文说明见仓库根目录的 [README.md](../README.md)。
