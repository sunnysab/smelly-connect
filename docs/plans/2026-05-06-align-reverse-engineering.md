# 对齐逆向结果实施计划

> **目标**: 将 smelly-connect 的协议实现与 svpnservice 逆向结果对齐，确保能与真实 EasyConnect 网关互通。

**分支**: `feat/align-reverse-engineering`

---

## 当前状态 vs 目标状态

| 组件 | 当前实现 | 目标（逆向结果） | 优先级 |
|------|---------|-----------------|--------|
| sslctx | 未提取 | 从 rclist.csp 解析 128 字符 hex → 64 字节 → key[16] + randnum[16] | P0 |
| 命令隧道 | 真实 TLS + 自定义握手 | 固定 TLS 字节流 + JJYY ClientMsg + AABB ServerMsg | P0 |
| 数据隧道 | 真实 TLS + 0x05/0x06 握手 | 固定 TLS 字节流 + type 5/6 ClientMsg + IPCP 帧 | P0 |
| 数据平面 | 裸 IP 包 over TLS record | IPCP 帧 (10B header) + RC4 加密 | P0 |
| 心跳 | ICMP ping | 命令隧道 ClientMsg(type=3) | P1 |
| conf.csp | 仅获取 TLS session | 解析 loginName, SvpnID 等元数据 | P1 |
| IP 回复解析 | reply[4:7] | AABB ServerMsg 字段 A | P0 |
| 压缩 | 无 | LZO/ZLIB（延后） | P2 |

---

## Task 1: sslctx 解析

**目标**: 从 rclist.csp 响应中提取 `sslctx`，解码为 64 字节，分割出 RC4 key 和 randnum。

**涉及文件**:
- 新建: `smelly-connect/src/kernel/tunnel/sslctx.rs`
- 修改: `smelly-connect/src/kernel/tunnel/mod.rs`
- 修改: `smelly-connect/src/kernel/control/parser.rs` (提取 Resource.Other.sslctx 属性)
- 修改: `smelly-connect/src/resource/model.rs` (ResourceSet 增加 sslctx 字段)
- 测试: 新增单元测试

**步骤**:

1. 在 `ResourceSet` 中增加 `sslctx: Option<String>` 字段
2. 在 `parse_resource_document` 中从 `<Other sslctx="...">` 提取 hex 字符串
3. 新建 `sslctx.rs`:
   - `decode_sslctx_hex(hex: &str) -> Result<[u8; 64]>` — 128 hex → 64 bytes
   - `split_sslctx(decoded: [u8; 64]) -> ([u8; 16], [u8; 16])` — (randnum[0x20..0x30], key[0x30..0x40])
4. 写测试：用已知 hex 值验证解码和分割
5. 将 sslctx 集成到运行时状态中

**验证**: 单元测试通过，能从 XML fixture 中提取 sslctx。

---

## Task 2: 命令隧道协议（JJYY/AABB）

**目标**: 替换当前的 TLS 握手 + 自定义消息格式，改用 svpnservice 的固定字节流协议。

**涉及文件**:
- 新建: `smelly-connect/src/kernel/tunnel/command.rs` — ClientMsg/ServerMsg 构建和解析
- 修改: `smelly-connect/src/auth/control.rs` — 替换 `connect_legacy_tunnel` 和 IP 请求逻辑
- 修改: `smelly-connect/src/kernel/tunnel/handshake.rs` — 重写消息构建
- 修改: `smelly-connect/src/kernel/tunnel/mod.rs` — 导出新接口
- 测试: 新增协议测试

**步骤**:

1. 实现 ClientMsg 构建器 (0x4C 字节):
   ```
   [0x17 0x03 0x01 0x00 0x3c] [00 00 00] [JJYY]
   [cmdType LE u32] [32B zeros] [16B sockaddr_in] [4B zero] [4B extra] [4B extra2]
   ```
2. 实现 ServerMsg 解析器 (0x28 字节):
   ```
   [AABB] [cmdType LE u32] [A] [B] [C] [D] [E]
   ```
3. 实现 `FixedSSLSyn` 和 `FixedSSLAck` 常量 (从 go-ecproto 复制 hex)
4. 实现命令隧道连接序列:
   - TCP connect → send ssl_syn(0x52B) → recv 0x7A → send ssl_ack(0x2B)
   - send ClientMsg(NEWCONNECT) → recv ServerMsg → 解析 SEND_IP
5. 实现心跳: 定时发送 ClientMsg(HEARTBEAT, type=3)
6. 替换 `request_ip_via_tunnel` 中的 TLS 逻辑
7. 写测试：构建/解析 ClientMsg、ServerMsg

**验证**: ClientMsg/ServerMsg 的构建和解析测试通过。

---

## Task 3: 数据隧道建立

**目标**: 用命令隧道协议建立 send/recv 数据隧道，替换当前的 TLS 方案。

**涉及文件**:
- 修改: `smelly-connect/src/auth/control.rs` — `open_send_tunnel`, `open_recv_tunnel`
- 新建或修改: `smelly-connect/src/kernel/tunnel/dataplane.rs` — 数据隧道连接逻辑
- 测试: 隧道建立测试

**步骤**:

1. 实现 send tunnel 连接:
   - 新 TCP → ssl_syn → ssl_ack → ClientMsg(type=5, extra=ntohl(tunIP))
   - 期望 ServerMsg.cmdType == 2
2. 实现 recv tunnel 连接:
   - 新 TCP → ssl_syn → ssl_ack → ClientMsg(type=6, extra=ntohl(tunIP))
   - 期望 ServerMsg.cmdType == 1
3. 替换 `spawn_legacy_packet_device` 中的隧道建立逻辑
4. 写测试

**验证**: 隧道建立逻辑测试通过。

---

## Task 4: IPCP 帧编解码 + RC4

**目标**: 实现 IPCP 帧格式和 RC4 加密，替换当前的裸 IP 包传输。

**涉及文件**:
- 新建: `smelly-connect/src/kernel/tunnel/ipcp.rs` — IPCP 帧编解码
- 新建: `smelly-connect/src/kernel/tunnel/rc4.rs` — RC4 KSA+PRGA 实现
- 修改: `smelly-connect/src/auth/control.rs` — `packet_device_from_tunnels` 改用 IPCP
- 修改: `smelly-connect/src/transport/device.rs` — 可能需要调整
- 测试: IPCP 编解码测试、RC4 测试

**步骤**:

1. 实现 RC4:
   - `RC4State { x: u8, y: u8, s: [u8; 256] }`
   - `new_rc4(key: &[u8]) -> RC4State`
   - `xor_key_stream(state: &mut RC4State, dst: &mut [u8], src: &[u8])`
2. 实现 IPCP 编码:
   - 10 字节 header: `0x17 0x03 0x01` + len_be + flags + reserved + orig_len_be
   - flags bit7=encrypted, low7=compress method
   - 顺序: compress → encrypt → frame
3. 实现 IPCP 解码:
   - 读 10 字节 header → 读 body → decrypt → decompress
4. 改造 `packet_device_from_tunnels`:
   - send 方向: IP 包 → IPCP encode (with RC4) → TCP write
   - recv 方向: TCP read → IPCP decode (with RC4) → IP 包
5. RC4 key 来自 sslctx[0x30..0x40]，send/recv 各自独立 state
6. 写测试：IPCP roundtrip、RC4 向量测试

**验证**: IPCP 编解码 roundtrip 测试通过，RC4 测试向量正确。

---

## Task 5: 集成 — 替换运行时控制平面

**目标**: 将 sslctx + 命令隧道 + 数据隧道 + IPCP 集成到运行时流程中。

**涉及文件**:
- 修改: `smelly-connect/src/runtime/control_plane/flow.rs`
- 修改: `smelly-connect/src/runtime/control_plane/types.rs`
- 修改: `smelly-connect/src/session/inner.rs`
- 修改: `smelly-connect/src/session/runtime.rs`

**步骤**:

1. `ControlPlaneState` 增加 sslctx 字段 (key, randnum)
2. `run_control_plane` 流程:
   - login_auth.csp → login_psw.csp → TwfID (已有)
   - fetch conf.csp (已有，但需解析元数据)
   - fetch rclist.csp → 提取 sslctx + 资源列表 (改造)
   - token 派生 (保留 zju-connect 方式作为备选)
   - 命令隧道连接 → SEND_IP (替换)
3. 数据平面建立:
   - 用 sslctx key 初始化 RC4 state
   - 建立 send/recv tunnel (type 5/6)
   - 创建 IPCP-aware 的 PacketDevice
4. 心跳改为命令隧道 type=3
5. 写集成测试

**验证**: 完整流程测试通过（如果有测试网关）。

---

## Task 6: conf.csp 元数据解析

**目标**: 从 conf.csp 响应中提取 loginName, SvpnID 等。

**涉及文件**:
- 修改: `smelly-connect/src/kernel/control/parser.rs`
- 修改: `smelly-connect/src/kernel/control/messages.rs`

**步骤**:

1. 实现 `parse_conf_metadata(body: &str) -> ConfMetadata`
2. 提取: `Conf.Other.login_name`, `Conf.Other.isRelogin`, `Conf.Service.SvpnID`
3. 写测试

**验证**: XML 解析测试通过。

---

## Task 7: 清理旧实现

**目标**: 移除不再需要的 TLS 相关代码。

**涉及文件**:
- 修改: `smelly-connect/src/protocol/legacy_tls.rs` — 可能移除或保留为备选
- 修改: `smelly-connect/src/auth/control.rs` — 移除旧的 TLS tunnel 代码
- 修改: `smelly-tls/` — 评估是否仍需要（可能保留用于 token 派生的 TLS 连接）

**注意**: `smelly-tls` 仍可用于 token 派生阶段的 TLS 连接（获取 ServerHello SessionId），但数据隧道不再使用它。

---

## 延后项目

- **压缩 (LZO/ZLIB)**: encType/zipFlag 非零时的处理，P2
- **UDP 数据通道**: 可选的快速路径，P2
- **svpnSessionID 轮换**: 服务器消息 type 4 的 session 刷新，P2
- **CSSL 资源代理通道**: 独立协议层，当前不需要

---

## 依赖关系

```
Task 1 (sslctx) ──┐
                  ├──→ Task 5 (集成)
Task 2 (命令隧道) ─┤
                  │
Task 3 (数据隧道) ─┤
                  │
Task 4 (IPCP+RC4) ┘

Task 6 (conf.csp) ──→ Task 5

Task 7 (清理) ──→ 最后执行
```

建议执行顺序: 1 → 2 → 4 → 3 → 6 → 5 → 7
