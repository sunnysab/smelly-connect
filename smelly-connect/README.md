# smelly-connect

`smelly-connect` 是工作区中的主 crate，负责 EasyConnect 的控制面、会话、路由、代理与 Rust 数据面接入。

当前已经支持：

- 用户名/密码登录
- 图形验证码回调
- 资源规则解析
- 获取分配 IP
- `connect_tcp()`
- 本地 HTTP 代理
- `reqwest_client()`
- ICMP keepalive

示例程序：

- `request_ip`
- `debug_route`
- `fetch_jwxt`

更完整的中文说明见仓库根目录的 [README.md](../README.md)。
