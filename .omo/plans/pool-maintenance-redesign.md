# pool-maintenance-redesign - Work Plan

## TL;DR (For humans)

**What you'll get:** 连接池状态从 7 种简化为 5 种（Idle/Connecting/Active/Dead/Disabled），维护逻辑从两条恢复路径合并为一条单一后台循环，请求路径不再阻塞在重连上。再也不会有 HalfOpen 节点无人管、请求线程被 probe 卡住的问题。

**Why this approach:** 混乱的根源是"恢复"这件事有两个主人（定时任务 + 请求路径），职责不清。让 maintenance loop 做唯一的主人，请求路径只消费 Active 节点、只上报失败——各自职责分明，状态机就自然清爽了。

**What it will NOT do:** 不改 smelly-connect 库的 Session 逻辑，不改 HTTP/SOCKS5 proxy 的 accept/forward 逻辑，不改配置结构。

**Effort:** Large
**Risk:** Medium — 状态机重构涉及 pool.rs 核心逻辑 + 测试适配

---

## Scope

### Must have
- 状态枚举从 7 压缩到 5（Idle/Connecting/Active/Dead/Disabled）
- 单一 maintenance loop 作为唯一恢复路径（三阶段：时间推进→补 deficit→健康探测）
- `acquire()` 请求路径瘦身：round_robin + Notify 200ms 等待，不做任何恢复
- `report_failure(name)` 统一接口：替代 4 个 report_live_session_*，Active→Dead 加倍退避
- `tokio::sync::Notify` 替换 50ms polling
- 删除散落的 `refresh_time_based_states`
- 删除旧的两条恢复路径代码（request-triggered probe, maintenance probe, ensure_min_pool_size 等）
- 删除 Suspect/HalfOpen/Open/Configured 状态
- 全部已有测试通过

### Must NOT have
- 不改 smelly-connect 库代码
- 不改 HTTP/SOCKS5 proxy 的 accept/forward 流控
- 不改配置结构
- 不保留 session 用于 transport rebuild

---

## 状态机最终版

```
Idle ──→ Connecting ──→ Active
  ↑         │               │
  │         │               │
  │         ▼               ▼
  │      Disabled         Dead
  │                         │
  └────── (backoff) ←───────┘
```

```
Idle:        账号就绪，等待被选中建连。无 session。
Connecting:  建连中。有 deadline 超时保护。
Active:      健康，持有 PooledSession，被 acquire 选中。
Dead:        不可用，退避中。backoff 加倍，到期后回到 Idle。
Disabled:    永久认证失败（密码错误），终态，永不离开。
```

---

## 实施步骤（按执行顺序分 4 波）

### Wave A — 数据结构（并发安全）

目标：只改类型定义，不改逻辑，保持编译通过。

**A1. AccountState 枚举重写**
- 5 变体：Idle / Connecting / Active(PooledSession) / Dead / Disabled
- 删除：Configured, Ready, Suspect, Open, HalfOpen
- 删除 AccountFailure 整个结构体
- 删除 PoolError::SessionConnectTimeout（不再需要）

**A2. AccountNode 精简**
- 保留：account, state, backoff, backoff_until, probe_in_flight
- 删除：reconnect_session, flaky_retry, consecutive_failures, failure_threshold, current_backoff, backoff_base, backoff_max, open_until, live_probe_in_flight

**A3. snapshot.rs 更新**
- PoolSummary: idle_nodes, active_nodes, dead_nodes, disabled_nodes, connecting_nodes
- selectable_nodes = active_nodes
- 保持 PoolHealthStatus (Healthy/Recovering/Down) 不变

**A4. state.rs / selection.rs 更新**
- state_label, build_pool_summary, next_backoff 适配新状态
- selection.rs 的 `next_selectable_index` 不变（predicate 由调用方传）

### Wave B — 核心逻辑（依赖 A）

**B1. maintenance_tick() 实现**
- 替换 `ensure_min_pool_size` + `run_periodic_maintenance_once` + `run_periodic_healthcheck_once`
- 三阶段：

```
Phase 1 - 时间推进:
  Dead & backoff_until <= now → Idle（backoff_until = None）
  Connecting & deadline <= now → Dead（backoff *= 2, backoff_until = now + backoff）

Phase 2 - 补 deficit:
  deficit = min_pool_size - count(Active) - count(Connecting)
  从 Idle 中挑 deficit 个 → Connecting（设 deadline = now + connect_timeout）
  spawn recover_account_session() 后台任务

Phase 3 - 健康探测（仅 keepalive_target 有值时）:
  遍历 Active，跳过 probe_in_flight=true 的
  spawn ICMP probe
```

**B2. 连接任务完成回调**
- 成功：Connecting → Active(session)，`notify.notify_waiters()`
- 永久认证错：Connecting → Disabled，`notify.notify_waiters()`
- 其他失败：Connecting → Dead（backoff *= 2），`notify.notify_waiters()`

**B3. acquire() 实现**
- 替换 `next_session()` + `next_live_session()` + `next_ready_with_session()`
- 逻辑：
  ```
  round_robin(Active) → 有就返回
  count(Connecting) > 0 → select! { notify 200ms }
    再试 round_robin → 有就返回
  503
  ```
- PoolState 加 `notify: tokio::sync::Notify` 字段

**B4. report_failure(name) 实现**
- 替换 `report_live_session_failure`, `report_live_session_unhealthy`, `report_live_session_reconnect_required`, `report_live_session_unhealthy_if_probe_fails`
- 逻辑：如果节点状态 == Active → Dead（backoff *= 2, backoff_until = now + backoff）
- 还需要一个 `report_connecting_failure(name, permanent)` 给 spawn 任务用（Connecting→Dead 或 Connecting→Disabled）

**B5. 删除旧代码**
- 删除：connect_one_configured, connect_one_configured_with_test_hook, claim_request_triggered_probe, claim_maintenance_probe, try_request_triggered_live_probe, complete_probe_success, complete_probe_failure, recover_and_complete_probe
- 删除：refresh_time_based_states 及其调用
- 删除：claim_live_session_probe, clear_live_session_probe

### Wave C — 调用方适配（依赖 B）

**C1. proxy/http.rs 适配**
- `handle_live_request`: `pool.next_live_session()` → `pool.acquire()`
- `handle_live_session_failure`: 改为 `pool.report_failure(account_name)`
- 删除对 `report_live_session_unhealthy_if_probe_fails` 的调用

**C2. proxy/socks5.rs 适配**
- 同上

**C3. runtime.rs 检查**
- 如果 PoolSummary 字段名变了，更新引用
- `effective_status` 逻辑不变

### Wave D — 测试适配（依赖全部）

**D1. tests/pool.rs 全面重写**
- 保留构造辅助函数（更新内部状态名）
- 每个测试用例按新行为重写断言
- 删除专门测试旧状态转移（suspect, threshold, half_open backoff 等）的用例
- 新增 maintenance_tick 行为测试

**D2. tests/status_command.rs 状态名更新**
- 匹配新状态输出

---

## 测试策略

- 每步 wave 完成后 `cargo build -p smelly-connect-cli --features test-utils` 检查编译
- Wave D 完成后 `cargo test -p smelly-connect-cli --features test-utils` 全通过
- 最终 `cargo build --workspace` 确保不波及其他 crate

## 提交

一次 squash commit：
```
refactor(cli): rewrite pool state machine and maintenance loop

- 5 states: Idle/Connecting/Active/Dead/Disabled
- Single maintenance_tick() loop
- acquire() with Notify 200ms wait
- report_failure() unified interface
- Removed 7 old states, request-triggered probe, refresh_time_based_states
