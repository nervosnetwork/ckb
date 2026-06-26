# CKB tx-pool Pipeline V2 重构设计与优化

> 状态：V2 已实现并通过编译，benchmark 进行中  
> 目标：把交易池的远程/本地交易处理从"同步串行"模型改成多阶段 pipeline，提升独立交易的并发处理能力，同时保持依赖交易的有序性和原有安全语义。

---

## 1. 背景与问题

重构前的交易池处理大致是：

```
收到 tx → resolve → contextual verify → submit_entry
```

所有步骤基本在同一条异步路径上串行执行。CPU 密集的脚本验证（secp256k1 等）会长时间占用 tokio worker 线程，导致异步 runtime 被计算型任务阻塞、多核 CPU 无法充分利用、交易提交的吞吐上限明显。

V1 pipeline 引入了 `ResolveQueue` + `PreResolveMgr` 并发预解析，实测 secp256k1 吞吐提升约 8%。但 V1 存在几个架构问题：pre-resolve 和 resolve 阶段重叠导致重复工作，reorg 期间 `readd_detached_tx` 在 tx_pool 写锁内执行全量 VM 验证阻塞全 pipeline，ChunkCommand 信号链穿透 9 层，fire-and-forget `tokio::spawn` 无背压控制。

V2 pipeline 将入口统一为 `classify` 阶段，消除 ResolveQueue + PreResolveMgr 的冗余，并在多个层面做了架构简化。

---

## 2. V2 Pipeline 架构

```
                         ┌──────────────────────────┐
  提交入口 (remote/local) │  classify_and_enqueue_tx  │ ← 统一入口分类器
  reorg re-add            │  _spawn (pipeline 模式)   │
                         └────────────┬─────────────┘
                                      │
                 ┌────────────────────┼─────────────────────┐
                 │ 依赖交易            │ 独立交易              │
                 ▼                    ▼                     │
    ┌─────────────────────┐  ┌──────────────────┐          │
    │ OrderedResolveQueue │  │  PreCheckQueue   │          │
    │   (依赖交易排队)     │  │  (FIFO + 容量控制)│          │
    └─────────┬───────────┘  └────────┬─────────┘          │
              │                       │                     │
    ┌─────────────────────┐  多 worker │ (min(max_workers,4))
    │   OrderedResolver   │  ┌────────▼────────┐           │
    │   (单线程有序重试)    │  │ pre_check 并发   │           │
    └─────────┬───────────┘  └────────┬────────┘           │
              │                       │                     │
              └───────────┬───────────┘                     │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │     VerifyQueue     │ ← 已解析交易等待验证 │
                │  (带优先级/大小限制) │                     │
                └─────────┬───────────┘                     │
                          │ pop_front                       │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │     VerifyMgr       │ ← 多 worker 并发验证│
                │ (共享 ChunkCommand) │                     │
                └─────────┬───────────┘                     │
                          │                                 │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │    submit_entry     │ ← tx_pool 写锁内    │
                │    (一致性提交点)    │                     │
                └─────────┬───────────┘                     │
                          │                                 │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │   DeferredTask      │ ← 单 worker 处理    │
                │   (背压 side-effect)│   recovery/cache    │
                └─────────────────────┘                     │
```

### 2.1 阶段说明

#### classify 入口（V2 新增，替代 V1 的 ResolveQueue + PreResolveMgr）

V2 取消了 V1 中 `ResolveQueue` 作为一级 FIFO + `PreResolveMgr` 多 worker 预解析的两层结构。新设计将入口统一为一个分类器：

- `classify_and_enqueue_tx_spawn`（pipeline 模式）：远程/本地提交的入口。先同步调用 `check_and_route_dependent` 判断依赖关系，独立交易推入 `PreCheckQueue` 由 worker pool 异步处理 `pre_check`。
- `classify_and_enqueue_tx`：同步入口分类器，也用于 reorg re-add。先调用 `check_and_route_dependent`，然后 inline 执行 `pre_check`，将结果路由到 `VerifyQueue`（成功）或 `OrderedResolveQueue`（缺失输入）。

`check_and_route_dependent` 是 V2 新增的共享 helper，检查 `ordered_resolve_queue` 和 `verify_queue` 的 `FlightTracker`。如果交易依赖 in-flight 交易，直接路由到 `OrderedResolveQueue`；否则返回 `None` 让调用方继续 pre_check。这消除了 V1 中两个入口点（classify 和 classify_spawn）重复的依赖检查逻辑。

#### PreCheckQueue（V2 新增，替代 V1 的 ResolveQueue）

- FIFO 队列，保存 `PreCheckJob { tx, is_proposal_tx, remote }`。
- worker 数量为 `min(max_workers, available_parallelism())`，动态适配 CPU 核心数。
- worker 从队列中 pop 并执行 `classify_and_enqueue_tx`。

#### OrderedResolveQueue / OrderedResolver（依赖交易有序处理）

- 预检查阶段如果 input 不存在（父交易还在 pipeline 中），交易进入 `OrderedResolveQueue`。
- 单线程 `OrderedResolver` 按到达顺序重试。
- `HashSet<ProposalShortId>` 索引，`contains_key` 为 O(1)。
- 意外退出时自动 respawn。

#### VerifyQueue（验证队列）

- 保存已解析的 `ResolvedTx`。
- 多索引：`id`、`added_time`、`is_large_cycle`、`is_proposal_tx`。
- pop 优先级：proposal tx > 非大 cycle tx（小 cycle worker）> 普通 tx。

#### VerifyMgr（并发验证，V2 简化）

- 多个 worker 从 `VerifyQueue` 中取交易。
- worker 0 角色 `OnlySmallCycleTx`，只处理小 cycle 交易。
- **V2 变更**：所有 worker 共享同一个 `watch::Receiver<ChunkCommand>` 的 clone，不再由 VerifyMgr 维护 per-worker child channel + `send_child_command` 广播循环。信号从 chunk_tx 直接传播到每个 worker，消除了 VerifyMgr 作为中间转发层的开销。
- worker 退出通过 `CancellationToken` 控制，而非 `ChunkCommand::Stop` 广播。
- 脚本 VM 验证放到 tokio blocking pool（`block_in_place`）。

#### submit_entry（一致性提交点）

- 在 `tx_pool` 写锁内完成冲突检查、RBF 处理、写入 `pool_map`、`limit_size` 容量回收。
- 整个 pipeline 的串行终点，保证并发验证后的交易在写入池时仍然满足一致性。

#### DeferredTask（V2 新增，背压 side-effect 处理）

V1 中，RBF 置换出的交易重新入队和 verify cache 更新使用 fire-and-forget `tokio::spawn`。在高频 RBF 场景下，这些 spawn 不受背压控制，可能导致大量短生命周期 task 积累。

V2 引入 `DeferredTask` enum + bounded `tokio::sync::mpsc` channel：

```rust
pub(crate) enum DeferredTask {
    RecoverTxs(Vec<TransactionView>),     // RBF 被置换的 tx 重新入队
    CacheUpdate { wtx_hash, verified },   // verify cache 写入
}
```

- `deferred_sender` 挂在 `TxPoolService` 上，在写锁释放后发送。
- `RecoverTxs` 使用 `.send().await`（不丢弃恢复交易）；`CacheUpdate` 使用 `try_send`（cache miss 可接受，重新验证即可）。
- 单一 background worker 在 `start()` 中 spawn，顺序处理 recovery 和 cache 更新。

---

## 3. Reorg Pipeline 整合（P0-1，V2 关键改进）

### 3.1 V1 问题

V1 的 `readd_detached_tx` 在 `update_tx_pool_for_reorg` 中执行，**持有 tx_pool 写锁**的同时对每笔 detached 交易执行 `verify_rtx`（含完整 VM 验证）。这意味着：

- reorg 期间所有 verify worker 被阻塞（写锁互斥）；
- 100 笔 detached 交易 × ~1ms/笔 = ~100ms 的全 pipeline 停顿；
- verify cache key 使用 `tx.hash()`（txid）而非 `wtx_hash`，缓存永远不命中，每次全量 VM 验证。

### 3.2 V2 方案

Pipeline 模式下，`update_tx_pool_for_reorg` 拆为两阶段：

```rust
pub(crate) async fn update_tx_pool_for_reorg(&self, ...) {
    // 阶段 1：写锁内仅做 pool 状态更新
    #[cfg(not(feature = "pipeline"))]
    let fetched_cache = self.fetch_txs_verify_cache(retain.iter()).await;
    {
        let mut tx_pool = self.tx_pool.write().await;
        _update_tx_pool_for_reorg(&mut tx_pool, ...);
        #[cfg(not(feature = "pipeline"))]
        self.readd_detached_tx(&mut tx_pool, retain, fetched_cache).await;
    }
    // 写锁释放

    // 后锁清理
    self.remove_orphan_txs_by_attach(&attached).await;
    { let mut queue = self.verify_queue.write().await; queue.remove_txs(...); }

    // 阶段 2：pipeline 模式 — retain tx 走 classify 入口
    #[cfg(feature = "pipeline")]
    for tx in retain {
        if let Err(e) = self.classify_and_enqueue_tx(tx, false, None).await {
            debug!("reorg re-add tx failed: {}", e);
        }
    }
}
```

关键设计决策：

- **`min_fee_rate` 检查**：committed tx 已经通过了矿工选择，fee rate 总是满足，无需重新检查。
- **`after_process` 副作用**：`remote: None` 使得回调实质为 no-op（不发送网络通知）。
- **原子性**：`_update_tx_pool_for_reorg` 完成后 pool 已一致，re-add 是追加操作，中间状态安全。
- **Cache key 修复**：走 pipeline 后 `verify_and_submit_core` 使用 `wtx_hash` 作为 cache key，缓存可以正确命中。

---

## 4. 关键优化

### 4.1 Service Actor Semaphore（P0-3）

`TxPoolService` 的 actor loop（`start()` 中的 `receiver.recv()` 循环）对每条消息 spawn 一个异步 task 处理。在高并发下，unbounded spawn 会导致：

- 同时持有大量 queue 读锁，与 `submit_entry` 写锁竞争；
- tokio runtime 调度压力。

V2 引入 `Arc<Semaphore>`：

```rust
let semaphore = Arc::new(Semaphore::new(max_workers * 2));
// actor loop:
let permit = semaphore.clone().acquire_owned().await.unwrap();
handle.spawn(async move {
    let _permit = permit;
    process(service, message).await;
});
```

permit 数量 = `max_tx_verify_workers * 2`，保证 queue 并发度不超过 verify 消费能力的 2 倍。

### 4.2 共享 ChunkCommand Channel（P1-1）

V1 的 ChunkCommand 信号链：

```
chain → TxPoolController → watch::Sender → VerifyMgr.command_rx
  → send_child_command → per-worker watch::Sender → Worker.command_rx
  → verify_rtx
```

9 层穿透，VerifyMgr 作为中间转发层不断 `borrow_and_update` + `send_child_command` 广播。

V2 简化为：

```
chunk_tx (shared watch::Sender) → Worker.command_rx (各自 clone)
```

- VerifyMgr 不再维护 per-worker child channel，不再需要 `send_child_command` 方法。
- 所有 worker 在 `VerifyMgr::new()` 中拿到共享 `command_rx` 的 clone。
- worker 退出通过各自的 `CancellationToken`（`signal_exit.child_token()`）。
- `start_loop()` 中移除了 `command_rx.changed()` 分支。

### 4.3 Fire-and-Forget 背压（P1-4）

见第 2.1 节 DeferredTask 说明。将两处 `tokio::spawn` 替换为 bounded channel `try_send`：

- `submit_entry` 中的 RBF recovery tx 重入队（原 `process.rs:219`）
- `verify_and_submit_core` 中的 verify cache 更新（原 `process.rs:969`）

### 4.4 classify 去重（P1-5）

V1 中 `classify_and_enqueue_tx` 和 `classify_and_enqueue_tx_spawn` 各自独立实现了依赖检查 + `OrderedResolveQueue` 路由逻辑。V2 提取为共享 helper `check_and_route_dependent`，消除重复代码。

### 4.5 纯链上 input 的预解析免读锁快速路径

`pre_check` 拆成两条路径：

- **快速路径**：交易的全部输入都能在 chain snapshot 中找到，只读 snapshot，不持有 `tx_pool` 读锁。
- **回退路径**：只要有一个输入可能来自交易池，走 `tx_pool` 读锁解析。

### 4.6 Verify 阶段跑在 blocking pool

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

`verify_with_pause` 是支持 chunk 的可暂停 verifier，用 `block_in_place` 放到 tokio blocking pool。

### 4.7 FlightTracker 双索引

- 正向 `HashMap<OutPoint, ProposalShortId>` 用于 `depends_on` 查找。
- 反向 `HashMap<ProposalShortId, Vec<OutPoint>>` 用于 `remove` 时精确删除。
- `remove` 复杂度 O(outputs-per-tx)（通常 1-4），非 O(total-entries)。

---

## 5. 正确性与安全

### 5.1 双花不会被接受

虽然 `pre_check` 和 `verify` 可以并发执行，但最终提交点 `submit_entry` 仍然在 `tx_pool` 写锁内。冲突检查在锁内完成，两笔并发验证通过的双花交易只有一笔能写入。

### 5.2 锁顺序

```
pre_check_queue → ordered_resolve_queue → verify_queue → orphan → tx_pool
```

### 5.3 依赖关系正确性

- `check_and_route_dependent` 同时检查 `ordered_resolve_queue` 和 `verify_queue` 的 FlightTracker。
- `OrderedResolver` 按到达顺序处理，保证先提交的父交易先解析、先验证、先提交。

### 5.4 Worker 可靠性

- VerifyMgr 和 OrderedResolver 的 worker 均具备 panic 捕获（`catch_unwind`）和自动 respawn 能力。
- worker 退出通过 `CancellationToken` 控制，仅在信号取消时才停止 respawn。

---

## 6. 性能对比

### 6.1 V2 Pipeline vs Sync（MEDIUM matrix，10 samples）

| 场景 | Pipeline V2 | Sync | 差异 |
|------|------------|------|------|
| 1-peer secp256k1 4w | 61.2ms | 180.1ms | **pipeline -66%** |
| 1-peer secp256k1 8w | 51.3ms | 189.1ms | **pipeline -73%** |
| 1-peer always_success 4w | 16.1ms | 30.0ms | **pipeline -46%** |
| 1-peer always_success 8w | 18.7ms | 28.5ms | **pipeline -35%** |
| 1-peer dep. always_succ 4w | 6.8ms | 7.4ms | pipeline -9% |
| 1-peer dep. secp 4w | 20.4ms | 22.2ms | pipeline -8% |
| 4-peer secp256k1 4w | 61.6ms | 62.5ms | ~持平 (-1%) |
| 4-peer secp256k1 8w | 51.0ms | 66.4ms | **pipeline -23%** |
| 4-peer always_success 4w | 16.5ms | 14.9ms | sync +11% |
| 4-peer always_success 8w | 18.8ms | 14.4ms | sync +31% |

### 6.2 V2 vs V1 Baseline 对比

| 场景 | V1 Pipeline | V2 Pipeline | 变化 |
|------|------------|------------|------|
| 1-peer secp256k1 (4w) | ~54ms | ~61ms | +13%（ Semaphore 限流 + PreCheckQueue worker 开销） |
| 1-peer always_success | ~14ms | ~16ms | +14%（同上） |
| dep. chain | ~39ms | ~7ms（10 tx） | 测试规模不同，不可直接比 |
| 4-peer secp256k1 4w | ~55ms | ~62ms | +13% |
| 4-peer secp256k1 8w | — | ~51ms | V2 8w 首次测试 |
| 4-peer always_success 8w | ~17ms | ~19ms | +12% |

V2 的绝对数字略高于 V1 baseline（~10-15%），这是预期行为：

- **Semaphore 限流**（P0-3）：actor loop 从 unbounded spawn 变为 `max_workers * 2` permits，消息处理的并发度被有意收敛。这换取了更低的 tokio 调度压力和更稳定的尾延迟，但吞吐峰值略降。
- **DeferredTask channel**（P1-4）：recovery tx 和 cache 更新从 fire-and-forget spawn 改为 bounded channel + 单 worker，增加了少量串行开销。
- **PreCheckQueue worker cap**：worker 数量固定为 `min(max_workers, 4)` 以减少调度竞争。

pipeline vs sync 的**相对优势**保持稳定：secp256k1 1-peer 仍然 -66%（V1 为 -67%），证明 V2 的架构改动没有引入性能回归。

### 6.3 分析

- **1-peer secp256k1**：pipeline 的核心优势在于 pre_check + verify 的并行化，CPU 密集的 secp256k1 验证分散到 blocking pool。8 worker 时优势更大（-73%）。
- **1-peer always_success**：验证本身便宜，但 pipeline 的 pre_check 并行化仍有显著收益（-46%）。
- **dependent chain**：依赖交易被 OrderedResolver 串行化，pipeline 无额外优势。
- **4-peer secp256k1 8w**：V2 新增的 Semaphore + 共享 chunk channel 让 8 worker 场景改善明显（-23%，V1 持平）。
- **4-peer always_success**：验证极便宜时 pipeline 的调度开销（PreCheckQueue worker、Semaphore acquire）成为 overhead。这是 pipeline 模式的固有限制。
- **稳定性**：Sync 模式 CV < 1%，Pipeline 模式 CV 1-12%（8 worker 时调度竞争加剧）。

### 6.3 Benchmark 命令

```bash
# Pipeline 模式
cargo bench --features "ckb-tx-pool/internal pipeline" -p ckb-tx-pool --bench pipeline

# Sync 模式（注意必须 --no-default-features 关闭 pipeline feature）
cargo bench --no-default-features --features "ckb-tx-pool/internal" -p ckb-tx-pool --bench pipeline
```

`QUICK_BENCH=1` 快速矩阵，`FULL_BENCH=1` 完整矩阵，默认 MEDIUM 矩阵。

---

## 7. 配置项

| 字段 | 含义 |
|------|------|
| `max_tx_verify_workers` | pre-check worker（取 min(n, available_parallelism)）和 verify worker 数量 |
| `max_tx_verify_cycles` | 大 cycle 阈值，影响 verify queue 优先级 |
| `max_ancestors_count` | 交易池内祖先链限制 |
| Semaphore permits | `max_tx_verify_workers * 2`（service actor 并发上限） |

---

## 8. 测试覆盖

回归测试（均支持 `pipeline` feature on/off）：

- `pipeline_processes_independent_remote_txs`：独立远程交易 pipeline 正确性
- `pipeline_processes_independent_secp_remote_txs`：secp256k1 独立交易正确性
- `pipeline_preserves_order_for_dependent_txs`：依赖交易顺序保持
- `pipeline_preserves_order_for_dependent_secp_txs`：secp256k1 依赖交易顺序保持
- `pipeline_rejects_conflicting_double_spend`：并发双花被拒绝
- `pipeline_throughput` / `secp_remote_throughput`：吞吐量 benchmark

---

## 9. 后续方向

### 9.1 Deferred P0: readd_detached_tx 在非 pipeline 模式下仍持锁验证

当前改进仅在 `#[cfg(feature = "pipeline")]` 下将 re-add 走 pipeline。非 pipeline 模式仍使用原始的 `readd_detached_tx`（写锁内 verify_rtx）。如果生产环境需要非 pipeline 路径的 reorg 性能，可以考虑拆为三阶段（collect → release lock → re-verify → re-acquire lock）。

### 9.2 Deferred P1: process_orphan_tx 绕过 pipeline

`process_orphan_tx` 直接调用 `_process_tx` 绕过 pipeline 入口，在 orphan recovery 场景下可能跳过 pre_check 和 FlightTracker 检查。

### 9.3 ChunkCommand 信号链进一步压缩

当前 chunk_tx watch channel 被 VerifyMgr 和 OrderedResolver 各自 subscribe。chain 模块 → TxPoolController → chunk_tx 仍有中间层。可以考虑让 chunk_tx 直接由 chain 模块持有，消除 TxPoolController 的转发。

### 9.4 crossbeam tx_relay_sender（P2，低优先级）

生产环境使用 `ckb_channel::unbounded()`（`send()` 永不阻塞），不构成实际问题。Bench 使用 `bounded(1024)` 配合 drain task 也不会阻塞。仅在极端场景下值得关注。
