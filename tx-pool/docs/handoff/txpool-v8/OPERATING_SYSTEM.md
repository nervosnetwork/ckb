# txpool-v8 工程运行手册

这是一份面向接手 AI 和人类维护者的统一导航。它把控制面、工程方法、执行纪律、证据边界、命令合同和恢复流程收拢到一处，但不另建事实源：发生字面冲突时，`MANIFEST.json` 绑定的 `HANDOFF.json`、`CONTROL_KERNEL.json`、`METHOD_LEDGER.json`、`AUDIT_PLAN.json`、`FINDINGS_LEDGER.json` 和源码拥有各自声明的权威。

## 1. 启动

1. 读取仓库根 `AGENTS.md` 与 `tx-pool/AGENTS.md`。
2. 按本目录 `README.md` 的顺序读取 manifest-bound 首读集。
3. 运行 `python3 tx-pool/docs/handoff/txpool-v8/VERIFY_HANDOFF.py`。
4. 验证失败时不得猜测、不得用聊天记录补状态；恢复精确 commit/tree 或内容寻址 checkpoint。
5. 验证通过后只从 `HANDOFF.json.next_action` 继续。

聊天、compact 摘要、轮次数、时间戳、未引用 checkpoint、退休 Partner 和未裁决 agent 结论都不是执行状态。

## 2. 当前事实

| 字段 | 字面值 |
|---|---|
| 实现基线 commit | `51d282345d1d83119c46cdde8f1115f14561b4ac` |
| 实现基线 tree | `1e19719c764c7349a178d7ac0b7bf4999542966f` |
| 当前 root | `B8_TRUE_SHARD_GLOBAL_TERMINAL_AUDIT_AND_ROOT_REPAIR_R1` |
| 普通生产 outer-write route | `0` |
| 路由迁移 | `COMPLETE_SCOPED_SYNCED_TO_MAIN` |
| 全局终审 | `OPEN_BLOCKING_CANDIDATES` |
| 性能排名 | `OPEN_NOT_AUTHORIZED` |
| 最终安全、复杂度、reviewer/Product Acceptance | `OPEN_OR_NOT_STARTED` |
| canonical 交付目标 | `G0_CURRENT_FROZEN_CANDIDATE_SET_NEXT_GENERATION` |
| 开放架构类研究 | `G0_OPEN_ARCHITECTURE_CLASS_RESEARCH_OPEN_FROZEN` |

“迁移完成”只表示普通 mutation 已离开 outer write arm；不表示终审、最小性、性能、安全或 Acceptance 完成。

## 3. 决策优先级

任何权衡严格按下列顺序：

1. 共识与数据完整性；
2. 敌对输入安全；
3. 静态所有权、确定性和可串行化；
4. 兼容与恢复；
5. 有界资源、性能和独立并发；
6. 可维护性与证明复杂度；
7. 便利性与源码体积。

后序属性不能补偿前序属性。CPU、LoC 或单一吞吐数字不是未经授权的硬阈值。

## 4. 架构硬约束

- 一个事实只有一个权威，一个对象只有一个生命周期位置。
- 状态改变遵循 `validate -> plan -> apply -> effects`。
- Plan 读取一个一致、明确有界的 cut，不做 I/O。
- Apply 重验 freshness，只提交最小原子改变。
- effects 在 authority 释放后运行，不能否决已提交改变。
- 无锁跨 `.await`；authority 临界区不做敌手规模的分配、克隆、析构或人口扫描。
- owner、资源、边、work capability、effect 和 wake 精确守恒。
- 普通生产 mutation 禁止全局、outer 或换名串行兜底。
- chain/generation/clear/close 只能为真实全局生命周期事实使用显式稀有 barrier。
- 禁止第二 policy/dependency/effect 引擎、第二 journal、扫描修复、watchdog、结果形状 flag、工具塑形生产重构。

## 5. 一轮工作的标准循环

1. **冻结对象**：commit/tree、命题、量词域、预期 discriminator、停止条件和输出位置。
2. **追因果链**：不可信输入 → 权威事实 → coherent read → plan premise → linearization point → effect/外部观察 → failure/resource/recovery。
3. **先收集**：先收集全部候选，不立即补丁；发现问题时只登记。
4. **归并根簇**：按一个权威事实、一个线性化点、一个外部观察聚类。
5. **Primary 裁决**：独立复现或反驳，记录最强竞争解释。
6. **先钉 canary**：每个 upheld blocker 先有确定性、无 sleep、生产绑定的失败见证。
7. **比较根**：至少比较一个最小 Rust-native 根和一个强替代，记录 TCB、状态、锁、任务、分配和兼容代价。
8. **一个自洽切片**：实现根因修复，同时退休被替代 route、adapter、测试模型、comment、checker 和文档。
9. **分层门禁**：focused → formatting/compile/lint → integration/full aggregate，仅在 owning boundary 运行。
10. **高视角复盘**：局部放大 + 全局贯穿，确认没有把代价转移到取消、失败、恢复、查询、关闭或维护面。
11. **持久化**：更新源码身份、claims、evidence、blockers、next action 和 manifest；需要时创建冷 checkpoint。

同一根第二次结构失败、出现补偿状态/fallback/换皮重试，或工作不能改变下一动作时，立即退出局部做高视角根因，不继续撞墙。

## 6. 证明模态

| 模态 | 能证明 | 不能替代 |
|---|---|---|
| `RUST_STATIC` | 所有权、可见性、线性 capability、穷举状态、谁能进入 Apply | trusted body 是否消费全部语义分量、开放类最优 |
| `SOURCE_BOUND_FORMAL` | 一个源码同构 slice 的命名量词与守恒 | 完整生产系统、架构类下界 |
| `FINITE_EXECUTABLE` | 当前生产 slice 的确定性反例、property、mutant、有限见证 | 一般 C、STATIC、G0 |
| `OPEN_CLASS` | 成员关系、一般下界、attainment、outsider | 不能由有限矩阵或标签闭合 |
| `INDEPENDENT_EMPIRICAL` | 冻结 binary/workload/environment/noise 下的比较 | 正确性、静态下界、未冻结候选 |

证据阶梯是：源码因果链 → 类型/锁图不可能性 → 确定性 canary → rebuild oracle/mutant → 可重放随机序列 → 小型 Loom/Shuttle → 命名形式证明。前一级已经决定命题时不为了仪式升级工具。

锁与并发使用可控的生产绑定证据栈：测试 seam 用 channel/barrier 排序，不用
wall-clock sleep；timeout 只作挂死守卫。C2 必须先持有真实 `read_all` guards，观察
真实 writer intent，再调用生产 nested routed read。测试态记录 `LockClass + shard +
mode` acquisition edge 并检查全局 DAG。`parking_lot/deadlock_detection`、Loom、
Shuttle 和 tracing 只作诊断或命名小量词的补充，不能单独证明无死锁。true-shard
资格另由两个 disjoint ordinary production Apply 在任一方释放前同时进入 final
exact-shard cut 的 canary 证明；稀有 lifecycle barrier 或 helper overlap 不算。

## 7. 审计纪律

- 审计前写 `AUDIT_PLAN.json`：对象、views、证据阶梯、成功/失败谓词。
- 每个候选都做局部状态机和全局传播：producer、consumer、observation、failure/resource、recovery、same-class surface。
- 全部候选收齐后再归因归并；禁止边发现边补丁。
- Partner、子 agent、报告和扫描器是中性输入，不是门禁。
- “未发现”不是一般证明；绿测试只证明 discovered universe。
- 安全审计使用架构消灭错误类 → 静态不变量 → 必要边界校验 → 有界恢复，不堆防御代码。
- 终审、性能、最终安全和 reviewer Acceptance 是不同阶段，不能互相替代。

## 8. 减法式最大工程努力

- 优先选择同时删除重复状态、交接、切口和代码的方案。
- 新类型必须拥有唯一不可合并的证明义务；仅翻译同一事实的 wrapper/carrier 合并或删除。
- 新任务/通道必须拥有独立生命周期或真实并发必要性，否则并回唯一权威切口。
- 每个性能/根设计裁决同时记录吞吐、CPU、延迟、authority cuts、唤醒、分配、生产 LoC、TCB、reviewer 认知成本、change amplification 和长期维护面。
- 先按全局收益和正确性杠杆排序；不先磨 1% 边界，也不因一次噪声轻易淘汰高价值根。
- profiling、benchmark、Tokio tracing/console 是维护组件；它们服务工程，不成为目标。
- 正确性与 integration 先于最终 benchmark。活性、TCB 收敛和并发 property/differential 证据闭合后，才冻结 binary/workload/environment/noise，并在独立与关联负载比较 `develop`；`develop` 只读，不做无关优化。

## 9. 外部与并行能力

Primary 是唯一全局计划、共享事实、独立复现、裁决和主仓集成者。

只有同时满足以下条件才分派：结果可能改变架构裁决或下一根；视角正交；能缩短关键路径且不制造同文件集成债；交付可由 commit/tree/command/canary/hash 重现；信息增益高于上下文与验收成本。

- 外部 Partner：fresh-eyes 长链审查、严格更小根、counterdesign、reviewer 说服力和机会成本；不是 gate 或主仓 implementer。
- 子 agent：冻结根上的正交深任务，如 Rust 类型/生命周期、竞态矩阵、TCB/复杂度或隔离原型。
- 不用 Partner/子 agent 做 lint、格式、机械检索；不投票决定正确性；Primary 不等待外部而停下关键路径。

## 10. 命令合同

```text
cargo fmt --all                 # 写格式
make fmt                        # 检查格式
make check                      # all-target compile check
make clippy                     # lint
cargo nextest run ...           # focused tests
make integration                # 集成边界
make test                       # 完整聚合；前面不跑 make quick-test
python3 -B tx-pool/scripts/check_all.py
git diff --check
```

禁止直接 `cargo test`、直接 `cargo clippy`。不同绝对源码根不得共用一个 `CARGO_TARGET_DIR`。独立 Nextest `LEAK` 是已知非阻断误报；真实失败、跳过必需测试和零匹配 selector 仍是 blocker。源码身份不变时不得重复 aggregate gate。

## 11. 文档与控制面

- `HANDOFF.json`：唯一 live phase/status/next action。
- `TODO.md`：Codex 中常驻的 manifest-bound 人类进度投影；不能覆盖 next action、audit execution order 或 contract phase order。
- `CONTROL_KERNEL.json`：优先级、硬不变量、禁区和命令合同。
- `METHOD_LEDGER.json`：经实践保留的方法；改变时必须删除/合并失效规则，禁止补丁雪球。
- `AUDIT_PLAN.json` / `FINDINGS_LEDGER.json`：当前终审计划与中性候选。
- `USER_REPORT_VALIDATION.json`：用户中性报告逐实例 Primary 裁决；报告与建议修复不是门禁。
- `CKB_AUTHORITY_INPUT_LEDGER.md`：用户权威输入；Section 9 是当前吸收状态，旧“当前处理”行只是历史快照。
- `DOCUMENT_AUTHORITY.json` / `DOCUMENT_AUDIT.json`：每份文档的角色与一致性证据。
- `architecture-contract.json`：稳定目标、硬约束、phase/Acceptance vocabulary，不是 live status。
- `HANDOFF.json.next_action`、`AUDIT_PLAN.json` 和 contract phase order：唯一有序执行计划；不保留重复 standalone plan。
- `security-regression-manifest.json` / `.release-progress`：可丢弃生成投影，不是 proof。

## 12. 断点、compact 与交接

每个物质根先有 immutable commit/tree 和必要 bundle，再写 checkpoint。checkpoint 只保存一个 current root、精确哈希、验证结果、blockers 和下一动作。

compact/重启后只加载首读集和命名源码切片，禁止回读聊天历史重建执行状态。`/private/tmp` 消失时只按内容寻址 bundle/checkpoint 恢复，不依赖用户转发。

一个 handoff 只有在以下条件同时成立时有效：

- 仓库 clean，精确 commit/tree 可解析；
- manifest 中每个文件哈希匹配；
- verifier 通过；
- 目标、量词域、claims、已闭合范围、OPEN blockers 和下一动作无歧义；
- 文档权威矩阵覆盖所有相关文档且无第二 current root；
- live agent/process 的未完成义务已明确登记；
- 新会话不需要聊天或旧 Partner 状态即可继续。

## 13. 当前下一动作

继续 `AUDIT_PLAN.json` 的 step 5–6：先读取 `FINDINGS_LEDGER.json`，对六个 blocker cluster 逐一做 Primary 确定性复现/反驳并记录 strongest counterexplanation；在完成聚类和 canary 前不修改生产语义。终审正确性闭合后，才进入 hard/static、性能、复杂度、最终安全和 Acceptance。
