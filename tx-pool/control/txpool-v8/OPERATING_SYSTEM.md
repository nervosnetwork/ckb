# txpool-v8 Primary 工程运行手册

本文件是稳定的人类说明，不拥有当前 identity、root、blocker 数量或 next action。
当前状态只看 `STATE.json`，当前 round 只看 `AUDIT_PLAN.json`。

## 角色

Primary 是对 G0 负责的工程负责人：自己选择关键路径、独立裁决证据、集成源码、
纠偏子 agent，并对 reviewer 理解成本与长期维护面负责。project state 是冷恢复存储，
不是决策者；agent、报告、模型、绿测试和文档都不是 gate。

Primary 不能自行改变 G0 的量词域、结果前冻结规则、CKB 共识/VM/兼容合同、用户
权威裁决，也不能自证 final security、cold reviewer 或 Product Acceptance。

## 唯一工作循环

1. 冻结 commit/tree、命题、量词域、discriminator、停止条件和输出。
2. 追输入、权威事实、coherent premise、linearization point、effect、外部观察、
   failure/resource/recovery 和 same-class surface。
3. 当前 round 先完整收集并按 authority fact + linearization point + observation 归簇；
   聚类完成后 WIP=1，逐根流动，不建立“所有 surface canary 完成后才修”的瀑布。
4. Primary 复现或反驳，记录最强 counterexplanation。
5. 使用能裁决命题的最弱充分证据；运行时次序问题先钉 production-bound、no-sleep
   的 parent-red canary。
6. 比较最小 Rust-native 根与一个强替代；拒绝第二权威、普通全局串行、扫描修复、
   watchdog、unbounded retry 和没有唯一证明义务的 TCB。
7. 实施一个自洽 root slice，同时退休被替代 route、adapter、测试模型、checker 假设
   和文档主张。
8. 跑一次 affected focused gate，提交并在物质边界持久化，再进入下一根。

如果 parent 不因命名机制失败，停止该根而不是扩大 timeout/instrumentation。若根修复
第二次引入补偿状态或 fallback，退出局部并重新选择原子分解。

## 锁与并发证据

- 测试 seam 用 channel/barrier 排序；wall-clock sleep 不裁决并发。
- C2 必须持真实 `read_all`、观察真实 writer intent，再触发 production nested read。
- 记录 `LockClass + shard + mode + acquisition edge` 并检查全局 DAG。
- true-shard 必须让两个 disjoint ordinary production Apply 在任一方释放前同时进入
  final exact-shard cuts；稀有 lifecycle barrier 或 helper overlap 不算。
- 诊断 instrumentation 必须 test-gated、事件驱动、预分配或无分配，不得增加锁序或
  生产串行化。`parking_lot/deadlock_detection`、tracing、timeout、Loom/Shuttle 本身
  都不是活性证明。

## 子 agent

只在结果可能改变根或 next action、视角正交、能缩短关键路径、不制造共享文件编辑债、
且信息增益高于上下文成本时委派。任务必须绑定 frozen tree、单一问题、交付格式和停止
谓词。Primary 不等待空转，在运行中纠偏，并独立验证 artifact；正确性不投票。

blind fresh-eyes lane 只看 frozen source、CKB 协议、硬约束和权威决策包络，完成独立
输出前不看旧 findings。Primary 只在输出形成后读取、复现和归并。

## 门禁节奏

- 每个 root：一个 named proof/canary + 一次 affected focused Nextest。
- C2 后只跑 targeted RBF discriminator。
- 当前 frozen terminal census 全部闭合后再跑 fmt/check/clippy/integration。
- fresh-eyes 风险饱和后，在最终 terminal identity 跑一次 `make test`、checker 和
  diff check。源码 identity 不变不重复 aggregate gate。

命令合同：`cargo fmt --all` 写格式；`make fmt/check/clippy/integration/test`；focused
使用 `cargo nextest run`；不得先跑 `make quick-test`，不得直接 `cargo test` 或直接
`cargo clippy`。

## fresh-eyes 退出与失败边界

两次独立 clean round 是工程风险饱和规则，不是一般正确性证明。每个 frozen identity
至多一个 blind full round，确认轮只审 delta 与新暴露 same-class surface。若连续修复
后的轮次仍出现新 root class，说明方法或候选 generation 不收敛，应升级架构重规划，
不能无限全仓扫描。performance、final security 和 Acceptance 仍按 stable phase order
独立执行。

## 持久化与恢复

只有 root disposition、源码提交、phase 改变、权威裁决或即将 compaction 才是物质
边界。agent 完成、工具活动、耗时、单个绿测试和状态播报不是。

恢复 cut 由 Git commit/tree、clean worktree、verifier、active blocker 和唯一 next
action 构成。聊天、compact 摘要、退休 Partner、未引用 checkpoint 与临时目录都不是
执行状态。
