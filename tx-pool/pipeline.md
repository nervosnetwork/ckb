# CKB tx-pool Pipeline 重构设计与优化

> 状态：已实现并跑通回归测试  
> 目标：把交易池的远程/本地交易处理从“同步串行”模型改成多阶段 pipeline，提升独立交易的并发处理能力，同时保持依赖交易的有序性和原有安全语义。

---

## 1. 背景与问题

重构前的交易池处理大致是：

```
收到 tx → resolve → contextual verify → submit_entry
```

所有步骤基本在同一条异步路径上串行执行。CPU 密集的脚本验证（secp256k1 等）会长时间占用 tokio worker 线程，导致：

- 异步 runtime 被计算型任务阻塞；
- 多核 CPU 无法充分利用；
- 交易提交的吞吐上限明显。

新的 pipeline 把交易生命周期拆成多个阶段，让 **resolve** 和 **verify** 可以并发进行，最后用单一提交点保证一致性。

---

## 2. Pipeline 架构

```
                    ┌─────────────────────┐
提交入口(remote/local)│    ResolveQueue     │ ← FIFO，保存原始交易
                    │   (Arc<RwLock<>>)   │
                    └─────────┬───────────┘
                              │ pop_front
                              ▼
                    ┌─────────────────────┐
            多 worker│   PreResolveMgr     │ ← 并发 pre_check / 预解析
                    │  (max_tx_verify_workers)
                    └─────────┬───────────┘
                              │ Ready / Orphan / Reject
              ┌───────────────┘
              ▼
    ┌─────────────────────┐
    │ OrderedResolveQueue │ ← 依赖交易按到达顺序排队
    │   (单消费者)         │
    └─────────┬───────────┘
              │ pop_front
              ▼
    ┌─────────────────────┐
    │   OrderedResolver   │ ← 顺序重试，保证依赖交易的输入可用
    └─────────┬───────────┘
              │
              ▼
    ┌─────────────────────┐
    │     VerifyQueue     │ ← 已解析交易等待验证
    │  (带优先级/大小限制) │
    └─────────┬───────────┘
              │ pop_front
              ▼
    ┌─────────────────────┐
    │     VerifyMgr       │ ← 多 worker 并发验证
    │  (含小 cycle 优先 worker)
    └─────────┬───────────┘
              │
              ▼
    ┌─────────────────────┐
    │    submit_entry     │ ← 在 tx_pool 写锁内完成冲突/RBF 检查与提交
    │    (TxPool 写锁)     │
    └─────────────────────┘
```

### 2.1 阶段说明

#### ResolveQueue（一级队列）

- 保存刚收到的原始交易（`ResolveJob`）。
- 维护去重索引 `ProposalShortId`。
- 维护 `total_tx_size`，超过阈值后拒绝新交易。
- 维护 `FlightTracker`，用于快速判断新交易是否依赖本队列中的交易。

#### PreResolveMgr（并发预解析）

- worker 数量由 `tx_pool_config.max_tx_verify_workers` 决定。
- 每个 worker 不断从 `ResolveQueue` 中 `pop_front`。
- 对每笔交易执行 `pre_check`：
  - 检查交易是否已存在；
  - 解析输入（优先走“纯链上 input 快速路径”，见第 3 节）；
  - 计算 fee、tx_size、status。
- 结果分三类：
  - **Ready**：进入 `VerifyQueue`。
  - **Orphan**：进入 `OrderedResolveQueue`，等待后续依赖可用。
  - **Reject**：直接回调 `after_process`。

#### OrderedResolveQueue / OrderedResolver（依赖交易有序处理）

- 预解析阶段如果 input 不存在（例如父交易还在 pipeline 中），交易不能直接进 `VerifyQueue`，否则会产生大量孤儿交易。
- 这类交易被送进 `OrderedResolveQueue`，由**单线程** `OrderedResolver` 按到达顺序重试。
- `OrderedResolveQueue` 内部维护 `HashSet<ProposalShortId>` 索引，`contains_key` 为 O(1)。
- 重试成功 → `VerifyQueue`。
- 仍然缺失 → 如果是远程交易进入 `OrphanPool` 并通知 relayer；本地交易直接拒绝。
- `OrderedResolver` 意外退出（panic 或异常停止）时会自动 respawn，保证 pipeline 不会部分瘫痪。

#### VerifyQueue（验证队列）

- 保存已解析的 `ResolvedTx`。
- 多索引：
  - `id`：唯一索引，用于去重和删除；
  - `added_time`：按提交时间排序；
  - `is_large_cycle`：大 cycle 交易标记；
  - `is_proposal_tx`：proposal 交易优先。
- pop 时优先级：proposal tx > 非大 cycle tx（小 cycle worker）> 普通 tx。

#### VerifyMgr（并发验证）

- 多个 worker 从 `VerifyQueue` 中取交易。
- worker 0 如果是 `OnlySmallCycleTx` 角色，只处理小 cycle 交易，避免大 cycle 交易阻塞小交易。
- 脚本 VM 验证被放到 tokio **blocking pool** 中执行（见第 3 节），不占用 async worker。
- 验证通过后调用 `submit_entry`。

#### submit_entry（一致性提交点）

- 在 `tx_pool` 写锁内完成：
  - 冲突检查（双花等）；
  - RBF 处理；
  - 写入 `pool_map`；
  - `limit_size` 容量回收。
- 这是整个 pipeline 的串行终点，保证并发验证后的交易在写入池时仍然满足一致性。

---

## 3. 关键优化

### 3.1 并发预解析（PreResolveMgr）

- 独立交易可以并行执行 `pre_check`。
- 预解析只读 snapshot / chain data，不修改交易池状态，因此可以多线程安全进行。
- 已解析交易（`ResolvedTx`）被直接推入 `VerifyQueue`，避免重复解析。

### 3.2 依赖交易有序重试（OrderedResolver）

- 如果直接让所有 worker 无差别地重试缺失 input 的交易，顺序会乱，子交易可能反复失败、反复进孤儿池。
- `OrderedResolver` 保证依赖交易按提交顺序处理，父交易一旦进池，子交易随即成功。

### 3.3 FlightTracker：避免无效解析

- `ResolveQueue` 和 `VerifyQueue` 内部维护 `FlightTracker`，记录当前队列中交易的输出 out-point。
- 新交易到达时先检查其输入是否命中 `FlightTracker`：
  - 命中说明它依赖正在 pipeline 中的交易，直接送进 `OrderedResolveQueue`，避免一次注定失败的预解析。
- 这减少了孤儿池抖动和重复 cell 解析开销。
- `FlightTracker` 内部维护双索引：正向 `HashMap<OutPoint, ProposalShortId>` 用于 `depends_on` 查找，反向 `HashMap<ProposalShortId, Vec<OutPoint>>` 用于 `remove` 时精确定位删除。`remove` 复杂度为 O(outputs-per-tx)（通常 1-4），而非 O(total-entries)。

### 3.4 Verify 阶段跑在 blocking pool

`tx-pool/src/util.rs` 中 pipeline 路径的验证：

```rust
block_in_place(|| {
    let handle = Handle::current();
    handle.block_on(async move {
        ContextualTransactionVerifier::new(...)
            .verify_with_pause(max_tx_verify_cycles, &mut command_rx)
            .await
    })
})
```

- `verify_with_pause` 是支持 chunk 的可暂停 verifier，本身是 async，但内部执行 CPU 密集型脚本 VM。
- 用 `block_in_place` 把这部分放到 tokio blocking pool，防止 async worker 被脚本验证占满。

### 3.5 纯链上 input 的预解析免读锁快速路径

`pre_check` 拆成两条路径：

- **快速路径**：交易的全部输入都能在 chain snapshot 中找到（即不依赖交易池中的任何交易），则只读 snapshot，不持有 `tx_pool` 读锁。
- **回退路径**：只要有一个输入可能来自交易池（pending/gap/proposed），再走原来的 `tx_pool` 读锁解析。

这样大量独立远程交易不会和 `submit_entry` 的写锁竞争读锁。

### 3.6 减少 submit_entry 持锁开销

- `_submit_entry` 的签名从接收 `TxEntry` 所有权改为接收 `&TxEntry`，避免提交阶段内部不必要的 clone。
- `limit_size` 中去掉每次 `pool_map.entries.shrink_to_fit()`，降低写锁内的内存整理开销。
- 这两处改动让写锁临界区更短，间接提升并发度。

### 3.7 容量控制与背压

- `ResolveQueue` 和 `VerifyQueue` 都维护 `total_tx_size`，超过阈值后 `Reject::Full`。
- `VerifyQueue` 区分大 cycle 交易，小 cycle worker 不会被大 cycle 交易饿死。
- `ResolveQueue::remove_txs` 采用批量 drain-rebuild 策略：收集所有待删除 id 后一次遍历完成，避免逐个删除的 O(n×m) 开销，在 chain reorg 等批量清理场景下尤为重要。

---

## 4. 正确性与安全

### 4.1 双花不会被接受

虽然 `pre_check` 和 `verify` 可以并发执行，但最终提交点 `submit_entry` 仍然在 `tx_pool` 写锁内：

- 冲突检查在锁内完成；
- 如果两笔并发验证通过的交易消费了同一 input，先拿到锁的一笔成功写入，后一笔在锁内发现冲突并被拒绝。

回归测试 `pipeline_rejects_conflicting_double_spend` 覆盖了这一场景。

### 4.2 锁顺序

为了避免死锁，pipeline 中的锁顺序约定为：

```
resolve_queue → ordered_resolve_queue → verify_queue → orphan → tx_pool
```

所有涉及多个 queue 的操作都遵守这个顺序。

### 4.3 依赖关系正确性

- 子交易在父交易真正可用前不会被提交；
- `OrderedResolver` 按到达顺序处理，保证先提交的父交易先解析、先验证、先提交；
- 本地交易缺失 input 直接拒绝，不会进入孤儿池。

### 4.4 Worker 可靠性

- `PreResolveMgr`、`VerifyMgr` 和 `OrderedResolver` 的 worker 均具备 panic 捕获和自动 respawn 能力。
- 每个 worker 通过 `catch_unwind` 捕获 panic，退出时通知管理器重生。
- `OrderedResolver` 的监控循环在 panic 或异常退出后自动重建新的 resolver 实例，确保依赖交易处理不会中断。
- 仅在收到全局退出信号（`CancellationToken` 取消）时才停止 respawn。

---

## 5. 性能对比

### 5.1 secp256k1 1-in-1-out 吞吐量（release，500 笔）

| 模式 | 吞吐 |
|------|------|
| sync（重构前） | ~2052 tx/s |
| pipeline（重构后） | ~2224 tx/s |

> 测试交易均为真实 secp256k1 签名，不是 always-success。

### 5.2 瓶颈分析

pipeline 提升幅度不大的主要原因是：

- 最终提交点 `submit_entry` 仍被 `tx_pool` 写锁串行化；
- 冲突检查、RBF、容量回收都在写锁内；
- 对于 1-in-1-out 独立交易，写锁提交时间占比很高。

因此继续优化的方向应该是**降低提交点的串行度**，而不是单纯增加 pipeline 阶段。

---

## 6. 已完成的优化与后续方向

### 6.1 已完成：FlightTracker 反向索引

- `FlightTracker::remove` 从 O(total-entries) 全表扫描优化为 O(outputs-per-tx) 精确删除。
- 新增反向索引 `HashMap<ProposalShortId, Vec<OutPoint>>`，`pop_front` 热路径不再扫描整个 HashMap。

### 6.2 已完成：ResolveQueue 批量删除

- `ResolveQueue::remove_txs` 从逐个调用 `remove_tx`（O(n×m)）改为一次 drain-rebuild（O(n)），chain reorg 批量清理更高效。

### 6.3 已完成：OrderedResolveQueue HashSet 索引

- `OrderedResolveQueue` 新增 `HashSet<ProposalShortId>` 索引，`contains_key` 从 O(n) 线性扫描降为 O(1)。

### 6.4 已完成：OrderedResolver 自动 respawn

- `OrderedResolver` 的监控循环在 worker panic 或异常退出后自动重建实例，避免 pipeline 部分永久瘫痪。

### 6.5 后续：queue 锁的进一步轻量化

- `ResolveQueue` / `OrderedResolveQueue` 本质上是 FIFO，可以考虑用 `crossbeam::channel` + `DashMap` 去重索引 + `AtomicUsize` 容量计数来替代 `Arc<RwLock<VecDeque>>`。
- `VerifyQueue` 带优先级和多索引，直接换成无锁结构较复杂；如果确认是瓶颈，可以拆成多个按优先级分发的 channel。
- 但当前 profiling 显示瓶颈在 `tx_pool` 写锁，queue 锁收益可能有限。

### 6.6 后续：提交点并行化

- 引入无锁的 conflict index（如 `DashMap<OutPoint, ShortId>`），让独立交易的冲突检查在写锁外完成；
- 只把真正冲突的交易串行化；
- 批量提交（batch submit）减少拿锁次数。

### 6.7 后续：Flight handoff 优化

- 当前交易从 `VerifyQueue` pop 后到 `submit_entry` 完成前，它的输出不在任何 `FlightTracker` 中；
- 子交易在此期间到达仍可能被误判为 orphan；
- 可以在 verifier 完成后、提交前把交易输出加入一个"待提交 flight 索引"，提交成功后再移除。

---

## 7. 配置项

相关配置在 `ckb_app_config::TxPoolConfig` 中：

| 字段 | 含义 |
|------|------|
| `max_tx_verify_workers` | pre-resolve 和 verify 阶段的 worker 数量 |
| `max_tx_verify_cycles` | 大 cycle 阈值，影响 verify queue 的优先级 |
| `max_ancestors_count` | 交易池内祖先链限制 |

---

## 8. 测试覆盖

新增的回归测试（均支持 `pipeline` feature on/off）：

- `pipeline_processes_independent_remote_txs`：独立远程交易 pipeline 正确性
- `pipeline_processes_independent_secp_remote_txs`：secp256k1 独立交易正确性
- `pipeline_preserves_order_for_dependent_txs`：依赖交易顺序保持
- `pipeline_preserves_order_for_dependent_secp_txs`：secp256k1 依赖交易顺序保持
- `pipeline_rejects_conflicting_double_spend`：并发双花被拒绝
- `pipeline_throughput` / `secp_remote_throughput`：吞吐量 benchmark

---

## 9. 总结

这次重构把交易池从同步串行模型改造为 **"并发预解析 → 有序依赖重试 → 并发验证 → 串行一致提交"** 的 pipeline：

- 独立交易可以并发解析、并发验证；
- 依赖交易通过 `FlightTracker` + `OrderedResolver` 保证有序和正确；
- 安全关键点仍集中在 `tx_pool` 写锁内，未引入新的双花或死锁风险；
- 脚本验证放到 blocking pool，避免 async runtime 被计算阻塞；
- 所有 worker（PreResolveMgr、VerifyMgr、OrderedResolver）均具备 panic 捕获和自动 respawn 能力；
- `FlightTracker` 使用双索引实现 O(1) remove，`ResolveQueue` 批量删除 O(n)，`OrderedResolveQueue` 使用 HashSet 索引 O(1) 查找；
- 实测 secp256k1 1-in-1-out 吞吐有约 8% 提升，但进一步提升需要优化提交点的串行化。
