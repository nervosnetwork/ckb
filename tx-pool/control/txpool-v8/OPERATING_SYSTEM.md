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

## 多维 typed-root 闭合

局部尺度只负责把一条 `producer -> receipt/premise -> consumer -> observation` 因果链
钉死；全局尺度必须从真实 production entrypoint 用 AST、符号索引和类型关系机械派生
调用图，并枚举 `route × cardinality 等价类 × stage × competing mutation`。production 与
`cfg(test)` route 必须分开；没有完成这些轴的清单，就不能因为分析文字抽象或单一路径
很深而自称“高视角”。第一个错误从 index 移到 dependency、resource、scheduler 或另一
cardinality/route 时，默认是根选择不完整，不能按 surface 逐个补丁。

实现前必须把同根错误分为三类：结构矛盾、typed prestate mismatch、subsystem stale。
只有 typed prestate mismatch 可以查询精确 sealed source receipt；source 已变化时返回 stale
或做合同允许的 canonical recapture，source 稳定时保留原 subsystem fault。结构矛盾不得被
宽泛改写成 stale，subsystem 自己已经证明的 stale 也不得绕回 source receipt。

证据成对建立：parent-red 固定根机制；参数化 production-bound matrix 覆盖冻结的四个轴；
稳定矛盾负控证明 fault 没被吞掉；最终 freshness/OCC 检查证明 Plan 证据不能越过 Apply。
若局部失败重复、跨 subsystem 迁移或来自未枚举轴，立即撤销当前根选择，取得一次
frozen-source fresh-eyes 强反设计，再比较最小 Rust-native 根与强替代；强替代必须至少
包含一个删除整个优化、route 或状态层的减法候选。机械修改按同根一次批量完成并复用
Cargo graph，禁止每遇到一个新错误就编译、补丁、再编译。

根在开始实施前写死退出条件：冻结 stage matrix、稳定 fault 负控、最终 freshness/OCC、
same-class AST/符号扫描和 affected focused gate。全部满足后立即 terminalize；未出现新
root class 时不得继续无界复审。

## 代价——必要？与完整候选

成本裁决分三层。第一层是必要义务闭包，只含 CKB 语义、已复现反例、资源/兼容硬约束
和 true-shard overlap；第二层是能端到端覆盖闭包的完整候选；第三层才把 production/proof
TCB、types、routes、locks、tasks、channels、cuts、allocation/wake/replan、性能与 reviewer
事实归因到 mechanism cluster。不得按文件或 nominal type 各自找理由，否则会产生一组
局部必要却无法组合的碎片。

候选成本必须包含 composition tax：route 边界、adapter、事实转译、error remap、receipt
merge、test-only 第二引擎和额外测试模型。Optional optimization 只有在固定 binary、
workload、environment、router layout 与 noise 下支付可重复的 performance rent 才能保留。
Develop 是成本基线，不是正确性模板；测试数量、历史实验和源码存在本身都不证明必要。

迁移以完整纵向 route 为单位：`entry -> policy -> plan -> support -> Apply -> effects`。
每个切片后系统必须可编译、可运行、可回退，同时删除 superseded route、测试模型和文档。
不能先删零件、最后再拼系统；也不能用一个薄 wrapper 隐藏第二语义引擎。

成熟工具优先于自造框架。按命题使用 rust-analyzer/ast-grep、Kani/Miri、Loom/Shuttle/
TSan、llvm-lines/bloat、llvm-cov、targeted mutants 与固定 binary profiling/A-B；工具结论
必须声明量词域。只有 repo 独有且成熟工具无法表达的回流约束，才允许一个最小 checker。

## 切片复盘与方法演化

每个 root slice 在 terminalize 前做一次轻量复盘，只回答四件事：什么证据改变了根或
next action；什么活动只有工具忙碌没有信息增益；什么误路、重复扫描/编译或 observer
effect 可以机械消除；所得经验是一次性事实、same-class tactic，还是跨根 stable method。

只有可迁移且有生产证据或强反设计支持的增量才进入 `METHOD_LEDGER.json` 或稳定
`CONTROL_KERNEL.json`。新规则必须合并、替代或删除旧规则；没有方法增量就记作
`NO_METHOD_DELTA`，不为复盘制造文档。方法更新后机械重建 method identity/projections
并跑 structural negative canaries。复盘、改文档和工具活动本身都不是 root 进度或 gate。

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
- 并发根的 focused gate 必须覆盖冻结的 route × cardinality × stage × competing mutation
  等价类，并包含稳定 fault 负控；单一路径的 stage matrix 不得外推到整根。
- `make integration` 默认只在当前 frozen terminal census 全部闭合后运行；若 live state 为
  某个 root 明确命名一次 observational integration，则在 focused closure 后只运行一次，
  结果只形成新证据，不推进 phase、不关闭其他 cluster、也不得 retry-until-green。
- 当前 frozen terminal census 全部闭合后仍须在最终 identity 跑 fmt/check/clippy/integration。
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
