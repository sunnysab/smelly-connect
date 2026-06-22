---
slug: pool-maintenance-redesign
status: awaiting-approval
intent: clear
pending-action: write .omo/plans/pool-maintenance-redesign.md
approach: 将 7 状态压缩为 4 状态，单一维护循环作为唯一恢复路径，请求路径只消费不恢复
---

# Draft: pool-maintenance-redesign

## 当前问题（Findings）

| # | 问题 | 代码位置 | 症状 |
|---|------|----------|------|
| 1 | 7 种状态过于复杂：Configured/Connecting/Ready/Suspect/Open/HalfOpen/Disabled | pool.rs:100-108 | 状态机难以推理，Suspect 与 Ready 实际行为没区别（都 selectable） |
| 2 | 两条恢复路径：maintenance timer + request-triggered sync probe | pool.rs:1082-1087 + 1898-1921 | HalfOpen 恢复有盲区，两条路径 cursor 互相干扰 |
| 3 | request-triggered probe 同步阻塞请求线程 | pool.rs:1905-1907 | 请求被挂起 connect_timeout（5s），高并发下堆积 503 |
| 4 | `refresh_time_based_states` 散落在每个公共方法入口 | pool.rs:852,877,895,1511,1625 等 | 每次持锁遍历所有节点，高频路径上造成锁竞争 |
| 5 | Open→HalfOpen 是隐式 timer 过渡，靠 polling 发现 | pool.rs:2056-2076 | 状态转换不直观，必须调用 refresh 才能看到最新状态 |
| 6 | Connecting 超时→Open（带退避）而非原地重试 | pool.rs:2065-2071 | 浪费一轮退避周期 |
| 7 | 无通知机制，请求路径用 50ms polling 等 Connecting 完成 | pool.rs:1642-1650 | 额外的锁开销，延迟恢复响应 |
| 8 | 健康检查只覆盖 Ready/Suspect，不碰 HalfOpen | pool.rs:1096-1113 | 即使有 keepalive_target，HalfOpen 节点也无人探测 |
| 9 | `live_probe_in_flight` 标记跨两条路径共享 | pool.rs:122,1171-1191 | 两条路径用同一把锁做防重，逻辑耦合 |

## 设计：状态机（4 状态）

```
Idle ──→ Connecting ──→ Active
  ↑         │               │
  │         │               │
  │         ▼               ▼
  │      Disabled         Dead
  │                         │
  └────── (backoff) ←───────┘
```

| 新状态 | 替换旧状态 | 含义 |
|--------|-----------|------|
| **Idle** | Configured + HalfOpen | 账号已配置但未建连；或退避到期等待下次尝试 |
| **Connecting** | Connecting（不变） | 正在建连中 |
| **Active** | Ready + Suspect | 会话健康，参与轮询。Suspect 不再存在——有失败就 Dead，不留中间态 |
| **Dead** | Open | 会话不可用，指数退避中，退避到期后回到 Idle |
| **Disabled** | Disabled | 永久认证失败（密码错误等），永不离开该状态 |

额外字段（不在状态枚举中）：
- `backoff: Duration` — 当前退避值
- `backoff_until: Option<Instant>` — 退避到期时间（仅 Dead 时有效）

## 设计：单一维护循环

所有定时状态转换、节点恢复、健康检查都由 **一个** maintenance loop 驱动。请求路径**只消费 Active 节点，不触发任何恢复**。

```
loop {
    // ── Phase 1: 时间推进 ──
    for node in pool:
        if node == Dead && deadline_expired(node):
            node = Idle            // 退避到期，可再次尝试
        if node == Connecting && deadline_expired(node):
            node = Dead            // 建连超时→Dead（不带额外退避，只是取消本次尝试）
            node.backoff = min(node.backoff * 2, max)  // 仅超时才加倍退避

    // ── Phase 2: 填满 Active 池 ──
    // target_active = min_pool_size
    // max_standby = min(3, total - target_active)  最多额外预备 3 个热备
    // to_fill = target_active + max_standby - count(Active) - count(Connecting)
    //
    // 跳过 Disabled 节点

    // ── Phase 3: 健康探测 Active 节点 ──
    if keepalive_target.is_some():
        for node in Active:
            if probe_in_flight: continue
            spawn icmp_ping(node)
            // ping 失败 → node = Dead（开启退避）

    sleep(interval)
}
```

**特性：**
- 不再有 HalfOpen→Connecting 的隐式转换，所有 Idle 节点平等竞争
- 维护的不仅是 `min_pool_size`，还有额外的 `standby` 槽位 → 即使 Active 没减少也持续恢复备用节点
- `spawn_recover()` 是后台任务，不阻塞任何请求

## 设计：请求路径

```rust
fn acquire() -> Result<Session, NoReady> {
    let node = round_robin(Active nodes)
    if let Some(n) = node {
        return Ok(n.session)
    }

    // 有节点正在连接？等通知
    if has_connecting() {
        notify.wait(Duration::from_millis(200))
        let node = round_robin(Active nodes)
        if let Some(n) = node {
            return Ok(n.session)
        }
    }

    stats.no_ready += 1
    Err(NoReady)
}
```

- 不触发 probe
- 不调用 `connect_one_configured`
- 不阻塞在连接恢复上
- 用 `tokio::sync::Notify` 替代 50ms polling
- **唯一副作用**：调用方上报失败 `report_failure(name)`

## 设计：失败上报

```rust
// 由 proxy http/socks5 在 upstream connect 失败时调用
fn report_failure(account_name) {
    node = Active → Dead（开启退避）
    // 不触发任何恢复——交给 maintenance loop
}
```

## 设计：刷新机制的改变

去掉散落在各处的 `refresh_time_based_states` 调用。改成：
1. 只有 maintenance loop 负责时间推进
2. `acquire()` 路径不做任何时间推进——它只检查 Active 列表
3. 唯一需要时效性的地方：Connecting 超时。用 `Notify` 通知请求路径，而不是靠 poll

## 迁移步骤

1. 重写 `AccountState` 枚举：4 状态 + 辅助字段
2. 重写 `maintenance_loop`：单循环、三阶段
3. 重写 `next_live_session()` / `acquire()`：瘦身，去掉所有恢复逻辑
4. 引入 `Notify`：替换 50ms polling
5. 去掉 `refresh_time_based_states`：从所有公共方法删除
6. 重写 `report_live_session_*` 函数：统一为 `report_failure`
7. 清理测试：适配新状态名称和预期行为
8. （可选）删除 `Suspect`、`HalfOpen`、`Open`、`Configured` 序列化兼容

## Scope IN

- 状态枚举重构
- 维护循环重构
- 请求路径瘦身
- Notify 通知机制
- 所有 `report_live_session_*` 合并简化
- 删除 `refresh_time_based_states`
- 适配对应测试

## Scope OUT

- 配置格式不变（已有 `min_pool_size` 字段）
- 外部 API（snapshot 序列化）保持向后兼容或一次性修改
- 不改变 smelly-connect 库本身的 Session 逻辑
- 不改 SOCKS5/HTTP proxy 的上层流控逻辑

## 开放问题

~~1. **退避策略**：Connecting 超时是否应该加倍退避？当前设计是「取消本次尝试，加倍退避」。也可以「超时不加退避，直接回到 Idle 等下次循环重试」。建议：超时不加退避，仅业务层面失败才加。~~
~~2. **standby 数量**：`max_standby = min(3, total - target_active)` 是否合适？~~

已决策：
1. **一律加倍退避**（Connecting 超时 + 业务失败都加倍）
2. **不维护 standby**：只维护 `min_pool_size` 个 Active。超出部分不恢复，直接留在 Idle 做冷备。当 Active 减少时，从 Idle 补足。
