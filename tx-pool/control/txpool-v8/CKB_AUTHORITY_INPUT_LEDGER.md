# CKB 权威决策输入账本

## 一、账本用途

这份账本只记录一种问题：**现有源码、测试、发布文档和历史证据都无法确定，但不同答案会改变 CKB 兼容承诺或生产设计的决策。**

它不是待办清单，也不是一般的技术讨论区。性能偏好、实现细节、证明方法、工具故障、测试失败、协议邮箱状态，以及可以通过源码实验解决的问题，都不得写入这里。

| 字段 | 当前值 |
|---|---|
| `ledger_id` | `TXPOOL_V8_CKB_AUTHORITY_INPUT_LEDGER_V1` |
| `scope` | `CKB_TX_POOL_PROTOCOL_COMPATIBILITY_AND_OWNED_DESIGN_INTENT_ONLY` |
| 当前待裁决条目数 | `0` |
| 当前状态 | `ALL_AUTHORITY_INPUTS_VERIFIED_AND_ABSORBED; ENGINEERING_TERMINAL_AUDIT_OPEN` |
| 是否阻塞全部工作 | `否` |
| 字面目标 `G0` | `OPEN` |

## 二、什么情况下才能登记

新增条目必须同时满足以下四项：

1. 问题涉及共识、网络协议、公开 API、持久化格式、配置兼容、恢复行为，或已有明确归属方的生产运行承诺。
2. 已检查当前生产源码、相关测试、发布文档和可验证的历史证据，仍无法确定预期行为。
3. 不同答案会实质改变生产实现、兼容承诺或外部可观察行为。
4. 不能通过更小的源码绑定实验解决，也不能在保持当前行为的前提下自然消除。

只要有一项不满足，就应回到普通工程分析，不得把问题升级为“等待权威输入”。

## 三、状态定义

| 状态值 | 准确含义 |
|---|---|
| `OPEN_NEEDS_AUTHORITY` | 源码证据无法确定设计意图，需要维护者、设计负责人或明确的新决策。 |
| `INPUT_RECEIVED_UNVERIFIED` | 已收到回答，但尚未核对出处，也尚未确认它对代码和兼容面的准确影响。 |
| `VERIFIED_AND_ABSORBED` | 回答已有可追溯出处；影响范围已核验，并写入不可变 checkpoint。 |
| `RESOLVED_BY_REPOSITORY_EVIDENCE` | 后续找到了足够的源码、测试或文档证据，不再需要外部裁决。 |
| `WITHDRAWN_NOT_AN_AUTHORITY_QUESTION` | 复审确认它只是工程或科学问题，不属于设计权威问题。 |

## 四、已裁决条目

本节保留每项裁决形成时的源码身份、checkpoint 和“当前处理”快照，用于说明决策来源，不充当当前执行状态。当前主仓吸收情况统一以第九节为准；发生冲突时，第九节和仓库内 `STATE.json` 优先。

### CKB-AUTH-0001：relay 邮箱溢出时，旧的精确结果是否必须丢弃

#### 需要裁决的问题

当前实现中，tx-pool 向 sync relayer 发布已提交的远程交易结果。relay 邮箱达到容量上限时，会丢弃队列中较早的逐笔结果，先写入 `GenerationReset`，再尽可能保留当前结果。

需要明确的是：

> 在不放宽内存、工作量、生命周期和关闭边界的前提下，后续实现是否可以保留更多、甚至全部逐笔结果；还是说“溢出时丢弃旧明细并通过 `GenerationReset` 重建投影”本身就是必须保留的运行语义？

这不是在问是否修改网络消息格式，也不是在问是否允许无界队列。两种选项都必须保留现有资源上限和单一消费者结构。

#### 为什么源码还不能给出最终答案

源码和测试已经证明：当前的丢弃与重整行为是有意设计，不是偶发缺陷。它们没有说明另一件事：**在资源和生命周期约束不变时，保留更多精确明细究竟属于允许的可用性增强，还是属于不兼容的行为变化。**

#### 证据位置

| 项目 | 内容 |
|---|---|
| 源码提交 | `d88c2a9644c1bab7a1a4ed60064a1ca7f7942f8f` |
| 发布端 | `/Users/zhangdingwei/projects/ckb/tx-pool/src/authority/relay.rs`，`AuthorityRelaySink::publish` |
| 消费端 | `/Users/zhangdingwei/projects/ckb/sync/src/relayer/mod.rs`，`Relayer::send_bulk_of_tx_hashes` |
| 关键测试 | `uak_relay_mailbox_overflow_orders_reset_before_the_current_result` |
| 容量测试 | `uak_production_relay_mailbox_fits_reset_and_one_maximum_parent_frontier` |
| 独立运行证据 | P4 单次运行在首个 300ms 消费时点之前触发 `NonExactDisposition`；该次运行依法没有形成性能结论 |

#### 可选裁决

**选项 A：保持当前语义。**

- 邮箱溢出时继续丢弃较早的逐笔结果，并用 `GenerationReset` 触发投影重整。
- 不把本次 P4 失效提升为产品修复任务。
- 优点是完全保留现有资源与运行行为；代价是突发负载下可能失去逐笔传播明细。

**选项 B：允许在相同或更强的资源边界内提高明细保留能力。**

- 允许重新组织内部批次、容量或单一消费者的唤醒方式，以保留更多或全部逐笔结果。
- 必须保持现有网络格式、结果顺序、`GenerationReset` 语义、缺失父交易恢复、内存上限、关闭路径和单一消费者。
- 不允许通过无界队列、阻塞 Apply、第二接收器、重试任务或全池扫描实现。

**选项 C：只有显式配置或协议版本允许时才能增强。**

- 当前没有证据支持这一选项。
- 若选择，必须同时指定默认行为、升级路径和兼容窗口。

#### 权威裁决

采用前述第一种目标：**有限资源下尽量减少逐笔明细丢失，但不承诺极端压力下逐笔无损。**

允许的优化范围如下：

1. 队列继续保持有界，且必须同时约束批次数、字节数、单次工作量和关闭路径。
2. 允许在现有 committed-effect 发布切口，将普通 `Ok/Reject` 结果整理为保持顺序的有界批次。
3. 允许既有的单一消费者在队列从空变为非空或达到高水位时提前排空；不得增加第二接收器。
4. `UnknownParents` 和 `GenerationReset` 仍是独立的顺序边界，不得被普通结果批次吞并或跨越。
5. 队列达到上限时，继续使用 `GenerationReset` 作为最终兜底；不得静默丢弃。
6. 在上述行为和硬约束完全相同的方案中，以性能最优为实现选择目标：同时降低发布端 CPU、分配、锁等待、通知次数和 reset 率，提高有效吞吐；不得以扩大无关内存、削弱语义或转移成本换取局部数字。

明确不采用“逐笔严格无损”目标，因此不得仅为了逐笔无损而引入无界队列、阻塞 Apply、第二接收器、重试任务、全池扫描，或新的提交前 relay 容量背压。

这项裁决允许在相同或更强的资源边界内提高 relay 明细保留能力；它不把 P4 失效变成性能结论，也不关闭 P4、产品验收或字面目标 `G0`。

#### 当前保守处理

| 字段 | 当前值 |
|---|---|
| `status` | `VERIFIED_AND_ABSORBED` |
| 裁决结果 | 允许“有界批处理 + 既有单一消费者提前排空 + `GenerationReset` 最终兜底”；不要求逐笔严格无损；在相同行为与硬约束下选择性能最优实现 |
| 被阻塞的工作 | `NONE` |
| 不受影响的工作 | 硬约束与静态证明研究、其他产品根、测试与文档纠正；P4 继续保持 `OPEN_FROZEN` |
| 权威输入 | 用户以项目决策归属方身份明确选择“第一种”目标 |
| 权威出处 | `USER_OWNED_DECISION_2026-08-23_RECORDED_IN_DURABLE_LEDGER` |
| 主任务（Primary）核验 | 裁决不修改共识或网络格式；保留有限资源、单一消费者、顺序屏障、关闭路径和 reset 兜底；不引入逐笔无损承诺；性能结论必须绑定冻结二进制、工作负载、环境和噪声规则 |
| 吸收结果 | 解除该产品设计的权威歧义；允许开展有界优化的设计与隔离实现，并以发布端成本、消费端成本、内存、吞吐和 reset 率的共同非劣化作为性能选择标准；不产生 P4、产品验收或 `G0` 闭合 |
| 对应 checkpoint | `00000000000000000243`（其 `MANIFEST.json` 绑定本账本版本） |

### CKB-AUTH-0002：字面目标 G0 的架构量词域与静态代价模型

#### 需要裁决的问题

字面目标 `G0` 要求找出“可证明全局静态最优”的 tx-pool 架构。要让这句话成为可证伪、可完成的工程命题，必须先明确：

> “全局”究竟量化哪些可实现架构；“静态最优”又在哪一种计算模型和代价偏序下比较？

这不是要求降低目标，也不是选择某个具体实现。它是在冻结定理的量词域。若不冻结，任何有限实现、形式模型或测试矩阵都只能证明局部最优；任何尚未构造的新算法又都可以被当作潜在局外人，因此 `STATIC_LOWER`、`STATIC_ATTAIN` 和字面 `G0` 永远没有可判定的闭合条件。

#### 已核验的事实

1. CKB RFC 和当前生产源码能够确定共识、交易结构、提议—提交窗口、公开接口和资源约束，但没有规定 tx-pool 内部架构的全局比较模型。
2. Herlihy–Wing 的线性化理论能够定义并发对象的正确历史，但不提供跨所有机器模型的实现最优序。
3. 经 Coq 修订验证的 Scalable Commutativity Rule 能在给定接口规范和机器模型下连接可交换性与无冲突实现；它明确依赖接口历史和机器模型，不能替项目选择全局代价序。
4. 事务内存的时间、空间和 RMR 下界都依赖严格串行化、progressiveness、读可见性、DAP、基元集合等前提。换一组前提，定理的适用类就会变化。
5. 当前仓库计划已经把 `S_floor` 写成分量下界，但 `A_H` 成员关系、计算模型和分量代价序仍未绑定；此前的有限正规形、表示包装和局部家族均不能补上这个量词缺口。

#### 一手依据

| 依据 | 与本条目的关系 |
|---|---|
| [CKB RFC 仓库](https://github.com/nervosnetwork/rfcs) | 冻结 CKB 协议与规范来源；不规定 tx-pool 内部全局架构类。 |
| [CKB Block Structure RFC](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0027-block-structure/0027-block-structure.md) | 冻结提议—提交窗口等链上硬语义。 |
| [Linearizability: A Correctness Condition for Concurrent Objects](https://www.cs.cmu.edu/~wing/publications/HerlihyWing90.pdf) | 定义并发对象历史的正确性与实时序。 |
| [A Revised and Verified Proof of the Scalable Commutativity Rule](https://arxiv.org/abs/1809.09550) | 证明结论依赖已给定的接口规范、历史与机器模型。 |
| [Progressive Transactional Memory in Time and Space](https://arxiv.org/abs/1502.04908) | 展示静态下界必须绑定一致性、进展、内存基元和代价模型。 |

#### 可选裁决

**选项 A（建议）：冻结“CKB 生产可实现架构类”。**

`G0` 的量词域定义为：

- 遵守当前 CKB 共识、网络、公开 API、存储、恢复和资源硬约束；
- 能在 CKB 支持的平台、Rust 工具链与运行时上实现；
- 只使用冻结的普通 RAM、原子操作、互斥、信号、任务和通道基元，或逐项列明并审计额外的 unsafe/FFI/硬件可信基元；
- 对外行为精炼到同一 tx-pool 历史规范；
- 静态代价采用分量偏序，不加权、不用词典序：语义权威数、必须排序的冲突支持、提交/否决切口、关键区人口工作、必要遍历次数、持久派生状态、任务/队列/通道/缓存角色、可信语义核、生产与证明复杂度；
- “全局”包括该冻结类中所有可构造竞争架构，不限于当前源码或已经实现的候选；模型外的新基元或新平台必须先作为新权威决策扩展量词域，不能在看到结果后暗中加入。

这会把 `G0` 变成“在明确的 CKB 生产实现宇宙内全局最优”，而不是只在两个候选或一个有限矩阵里最优。

**选项 B：量化所有数学上可能、未来可能出现的架构。**

该选项保持最宽的字面含义，但没有有限的成员判定、计算模型或搜索闭合条件。采用它等于明确接受：字面 `G0` 只能永久保持 `OPEN`，项目可以继续做生产优化，却不能科学地宣称全局静态最优。

**选项 C：另行给出明确的架构类和代价模型。**

需要逐字写出允许的机器基元、进展条件、故障模型、成员判定和分量偏序；仅写“所有合理实现”“现实可用实现”或“Rust 能实现的方案”不够精确。

#### 权威裁决

采用选项 A，并增加一条不可省略的候选资格：**最终架构必须能够工程实现。**

数学关系、下界、交换律和形式模型只作为证明骨架。一个数学点只有同时满足下列条件，才能进入最终候选集：

1. 能映射到明确的 CKB 生产类型、状态转移、接口和生命周期；
2. 能在 CKB 支持的平台、Rust 工具链和运行时上构造；
3. 所依赖的原子操作、互斥、通道、任务、unsafe、FFI 或硬件基元都已逐项声明，并纳入正确性、资源和复杂度核算；
4. 能给出生产实现或可复现的构造见证，而不只是存在性叙述；
5. 不通过削弱共识、安全、兼容、恢复、资源上限或关闭语义取得更低静态点。

因此，`G0` 的“全局”不是“当前源码中的几个候选”，也不是“不受机器模型约束的所有数学想象”，而是：**冻结的 CKB 生产可实现架构类中的全部合法架构。** 新平台或新可信基元若能实际落地，可以通过新的显式决策扩展该类；不得在看到比较结果后暗中改变成员规则。

“最大工程努力下的安全与性能最优权衡”按下列可检查规则解释：

1. 共识、数据完整性、敌对输入安全、兼容、恢复、确定性、有限资源和关闭语义是硬约束，不进入加权交换；任何退让都直接失去候选资格。
2. 在硬约束可行集内，先求静态结构的分量底；只有达到静态底的生产构造才能进入真实性能比较。
3. 性能比较必须绑定同一冻结二进制、工作负载、环境、指标、实际非劣界和噪声规则。候选必须在全部受保护场景和指标上同时实际非劣，不能用一个吞吐数字掩盖延迟、内存、CPU、锁竞争、恢复或关闭退化。
4. 性能等价的最强集合中，再选择工程实现最简洁、最清晰、最容易维护的架构。这里的“最简”不是代码行数最少或数学对象最少，而是语义权威、可变状态、生命周期角色、控制流分支、可信语义核、依赖和跨模块耦合都只保留完成合同所必需的部分；复杂度迁移、隐藏可信核和把成本转移到未测内存或后台任务都不算减少。
5. “最大工程努力”不按工时或轮次数定义，而以搜索闭合定义：每个主要 profile 成本中心都有一个可执行优化或一个明确的硬/静态阻塞；至少两次相互独立的 fresh search 在同一冻结身份上不再发现新的可执行 seam；所有幸存候选都已实际实现、完整通过工程门并在同环境比较。
6. 若仍存在不可消除的跨指标或跨场景交叉优势，则只得到 Pareto 集，不能宣称唯一全局最优。只有新的支配架构，或项目归属方事先冻结明确的业务取舍，才能把它收敛为单一结果。

“最安全”采用架构消除优先的静态保障层级：

1. 首先通过单一事实权威、最小原子提交、`validate → plan → apply → effects`、线性能力、密封构造和不可表示非法状态，从架构原理上消灭整类错误；例如原子化提交应使重入、半提交和中间态观察类错误失去可表达路径。
2. 再用 Rust 类型系统、生产绑定性质、模型检查或形式证明验证这些不变量。静态分析工具服务于生产不变量，不得成为第二套业务语义权威。
3. 运行时校验只放在外部不可信输入边界、资源边界和确实无法静态决定的条件上，并且必须有确定的成本和失败语义。
4. 日志、告警、reset、重试或降级只承担有界观察与恢复；它们不能替代架构消除和静态证明。

因此，增加 flag、watchdog、fallback、扫描、重复校验或防御分支本身不构成更高安全性。候选的安全裁决按“架构消灭错误类 → 静态证明不变量 → 必要的信任边界校验 → 有界恢复”排序；后一层不能弥补前一层本可消除却仍被保留的风险。

工程简洁性的裁决同时检查：是否只有一个事实权威和一个对象生命周期位置；类型与模块边界能否直接表达不变量；代码是否局部可推理；故障是否容易定位；测试是否直接绑定生产合同；修改一个语义是否只影响一个拥有者；依赖、构建、验证和运行维护负担是否最低。允许为了清晰增加少量类型、注释和直接性质测试；禁止为了减少行数而隐藏不变量、合并不相干职责或删除必要证据。

相应的选择顺序是：**硬约束可行 → 静态分量底 → 实测性能最强类 → 工程实现最简洁、最清晰、最容易维护。** 证明复杂度作为可维护性和可信面的一部分核算，不单独凌驾于工程实现。这不是把数学置于工程之上；数学只负责证明边界，最终结果必须是经过生产构造和真实性能验证的工程实现。

#### 当前保守处理

| 字段 | 当前值 |
|---|---|
| `status` | `VERIFIED_AND_ABSORBED` |
| 裁决结果 | 采用 `A`；最终候选必须工程可实现；安全等硬属性不可与性能交换；安全优先通过架构原理消灭错误类并由静态不变量证明，而非堆砌防御代码；按“硬约束可行 → 静态分量底 → 实测性能最强类 → 工程实现最简洁、最清晰、最容易维护”收敛；数学只作证明骨架，必须绑定生产映射和可构造见证。 |
| 被阻塞的工作 | `NONE`（量词域定义已解除；具体下界、达到、性能、复杂度和 Acceptance 仍需实际证明）。 |
| 不受影响的工作 | 全部生产工程、静态证明、候选攻击、性能与复杂度工作。 |
| 权威输入 | 用户以项目目标与设计决策归属方身份明确要求“最终目标肯定要能工程实现”，将工程意图表述为“在做最大工程努力的情况下，安全、性能都是最优的权衡实现”，明确“最小最简是工程实现概念：在性能最优、最安全的情况下，代码最简洁、最清晰、最容易维护”，并要求静态分析把安全提高到可证明不变量和架构原理级别，而不是依赖堆砌防御代码。 |
| 权威出处 | `USER_OWNED_DECISION_2026-08-23_RECORDED_IN_DURABLE_LEDGER` |
| 主任务（Primary）核验与吸收结果 | 冻结 `CKB_PRODUCTION_REALIZABLE_ARCHITECTURE_CLASS`；把“最大努力”改写为可检查的搜索闭合；把安全等早序属性固定为硬约束，并采用“架构消灭错误类 → 静态证明不变量 → 必要的信任边界校验 → 有界恢复”的保障层级；性能采用跨冻结场景的实际非劣强类；交叉权衡只形成 Pareto 集；最终同级候选按工程结构简洁、清晰和可维护性裁决，而非按代码行数或抽象数学大小；不把范围缩成当前源码或有限候选；模型外基元必须显式扩类；`STATIC_LOWER`、`STATIC_ATTAIN`、Acceptance 和 `G0` 仍为待证，不因本裁决自动闭合。 |
| 对应 checkpoint | `00000000000000000246`（其 `MANIFEST.json` 绑定本账本版本） |

#### 补充裁决：总 CPU 不是固定比例的自动淘汰门

项目归属方在最终候选 R5 尚未执行、尚未观察任何 R5 结果时，进一步明确：并行架构为了缩短墙钟时间而使用更多总 CPU 是正常且可能合理的工程取舍；项目从未要求“候选总 CPU 相对 current 的回退不得超过 10%”。此前最终对决合同里的 `max_target_cpu_ratio = 1.10/1.15` 是 Primary 自行预注册的工程阈值，不是 CKB 协议约束，也不是已获授权的 G0 目标函数。

因此，后续性能选择按以下规则解释：

1. `target_cpu_ns` 必须继续在相同二进制、工作负载、环境和 AB/BA 位置下完整测量、做比率和重复性报告，并继续用于 profiling 与工程优化。
2. 固定的 CPU 回退比例只作诊断参考，不得单独判 current 胜、候选失败或整场噪音失效。
3. CPU 只有在违反已声明的资源上界，或导致饱和、稳定性、公平性、关闭语义、受保护负载进展发生可复现退化时，才成为硬失败。
4. 若 profiling 已给出能够无损移除、且收益足以证明值得实现的 CPU 根，则“最大工程努力”仍要求先处理或明确冻结该根；不得把“CPU 非排名门”误读成“不再优化 CPU”。
5. 吞吐、延迟、内存有界性、分配、恢复和受保护工作负载仍按各自预注册合同裁决。跨指标仍存在真实交叉优势时，只能形成 Pareto 事实，不能用一个数字冒充唯一全局最优。

这项补充裁决在任何 authority-corrected R5 测量之前冻结；旧 R5 合同 `087275ab94bb750728845ce986c3b3ace0b45b68c765bd631b357cac91b62d1b` 从未执行、未观察结果，已标为 `SUPERSEDED_PRE_RESULT_NEVER_EXECUTE`。新合同必须显式把 CPU 标为“测量但不参与自动排名”，且不得改变生产身份、工作负载、其他指标、噪音规则或复用旧样本。

| 字段 | 当前值 |
|---|---|
| `status` | `VERIFIED_AND_ABSORBED` |
| 权威输入 | 用户明确说明：“总 CPU 约高 23% 很正常；从来没有要求 CPU 消耗不能高；它仍是可以优化的点。” |
| 权威出处 | `USER_OWNED_DECISION_2026-08-26_RECEIVED_PRE_R5_RESULT` |
| 吸收结果 | CPU 保持完整测量、解释与优化，但固定比例不再是自动排名门；只有实际资源/稳定性合同违约才构成硬失败。 |
| 预结果纠正物料 | `/Users/zhangdingwei/.codex/state/txpool-v8/materials/final-showdown-r5-authority-control/R5_AUTHORITY_CORRECTION.json`，SHA-256 `cdf3f163cb9bd3a7b0d4c3a14cc5e5ca3a635099afe9a2c5a4a4a4360b86b5ea` |

### CKB-AUTH-0003：是否把新的并发有序索引与内存回收可信基元纳入 G0 架构类

#### 需要裁决的问题

当前冻结的 `G0` 架构类允许普通 Rust 所有权、原子操作、互斥锁、读写锁、任务、信号和有界通道；额外的 unsafe、FFI 或硬件基元必须先逐项声明并审计。

现在需要明确：

> 是否允许 tx-pool 为并发有序索引引入新的第三方内存回收可信基元，并把它纳入后续静态达到与工程候选比较？

本条目只决定候选资格和可信基元边界，不决定采用哪个数据结构，也不授权把一次绿色实验写成 `STATIC_ATTAIN` 或 `G0` 闭合。

#### 为什么现有证据不能代替裁决

当前隔离实验已经证明：`scc::TreeIndex` 能表达线性化的并发有序 `insert/remove/range`，并通过 Accepted 到期索引的五个聚焦测试。但它同时引入 `scc 3.8.6`、`sdd 4.8.9`、`saa 5.6.0` 及新的 unsafe 并发内存回收可信面；其插入接口也没有保留现有 Plan 阶段的可恢复容量预留语义。

因此，实验只能证明“这类实现可构造”，不能替项目决定是否扩大冻结的 primitive/TCB 范围。实验源码和依赖已经从候选仓库完全回退，未进入主仓，也未进入任何性能或静态达到结论。

#### 证据位置

| 项目 | 内容 |
|---|---|
| 主仓身份 | `e1dc258d8e294ccdbdaa2f996484c1290955b2f5` / tree `a60026be877a947bc3ef4d03f5e3009419d52d81`，未修改 |
| 隔离最终身份 | `e0f8aa5f040ef420b2d6aafe4bf1b1b71d75c8fd` / tree `5d561e0808bad9fa4c217f607756e4beaf41107b`，最终不含 `scc/sdd/saa` |
| 路线阻断物料 | `/Users/zhangdingwei/.codex/state/txpool-v8/protocols/static-dap-dynamic-root-support-blocker-r1-stable-v1` |
| 物料 manifest SHA-256 | `589752694bfffcd8bbd49fa304e4978ed52f59dcc410f90a66a5c0beffce2671` |
| 有序索引 probe | `scc 3.8.6`；聚焦 Nextest run `b99db2fa-6e91-4c3f-ad83-35f10fe2a347`，5/5；仅作不可准入的构造性探针 |
| 精确冻结原因 | 新 primitive/unsafe TCB 未被当前冻结合同预先声明；同时缺少与现有可恢复分配、资源公式、回收和支持目标一致的完整审计 |

#### 可选裁决

**选项 A：不扩大 primitive/TCB 范围。**

- 后续候选只能使用当前冻结的普通 Rust 基元，或使用项目内能够完整审计、具有可恢复分配和有界回收证明的安全构造。
- `scc/sdd/saa` probe 永久留在“不可准入的构造性证据”，不得进入候选、性能比较或静态达到。
- 当前动态 fact-domain 路线继续保持 `OPEN_FROZEN`，除非出现不扩基元也能满足同一合同的新构造。

**选项 B：允许把 `scc/sdd/saa` 作为显式第三方可信基元候选。**

- 只能绑定预先冻结的精确版本、源码哈希、许可证、支持目标和依赖图。
- 必须先完成 unsafe/内存回收、线性化、取消与 panic、回收峰值、remove-heavy 工作负载、同步方法阻塞、构建与供应链面的审计。
- 必须明确解决或保持现有 Plan 阶段可恢复容量预留的等价失败语义；不得把进程级分配失败、后台延迟回收或不受控结构扩展藏在 Apply 内。
- 审计通过只取得“可进入候选”的资格，不产生性能、静态达到或 `G0` 结论。

**选项 C：允许另一项明确命名的并发有序索引基元。**

- 必须先给出精确 crate/版本或项目内实现、原语语义、内存回收模型、分配失败语义、目标平台和审计范围。
- 不能用“任意成熟并发容器”“之后选择合适实现”等开放表述。

#### 权威裁决

当前不扩大可信基元范围，冻结 `scc/sdd/saa` 路线。只有满足下面的重新激活条件时，才允许重新评估：

1. 在冻结的生产身份、真实工作负载、环境、指标和噪声规则下，profiling 明确把当前 `BTreeMap` 的 `insert/remove/range` 锁竞争识别为主导性能瓶颈；
2. 归因必须落到真实生产调用栈和锁等待，不能只凭微基准、容器理论特性或单次采样；
3. 重新激活只给予候选审计资格，不表示允许采用。候选仍须证明性能跨受保护场景实际非劣、内存与延迟回收有界、线性化与取消/panic/关闭正确、remove-heavy 工作负载稳定、现有可恢复容量预留和失败语义等价，并完成精确版本源码、unsafe、依赖与供应链审计；
4. 若 profiling 未指向该锁，或存在不扩大 TCB 的更小生产修复，则保持冻结，不得为了“可能更快”而引入依赖。

这项裁决不禁止今后基于新证据重新登记；它禁止在证据出现之前让并发容器试验占用 G0 主线。

#### 当前保守处理

| 字段 | 当前值 |
|---|---|
| `request_id` | `CKB-AUTH-0003` |
| `status` | `VERIFIED_AND_ABSORBED` |
| 所属子系统 | `CKB_TX_POOL_STATIC_ARCHITECTURE_AND_TRUSTED_PRIMITIVE_UNIVERSE` |
| 影响面 | 内部生产架构、支持平台、依赖与可信语义核；不修改共识、网络格式、公开 API 或存储格式 |
| 默认保守动作 | 不引入或使用新的并发内存回收可信基元；保持动态 fact-domain 路线 `OPEN_FROZEN` |
| 重新激活条件 | 冻结生产身份的 profiling 将当前 `BTreeMap insert/remove/range` 锁竞争识别为主导瓶颈，并绑定真实生产栈、工作负载、环境和噪声规则 |
| 被阻塞的工作 | 在重新激活条件满足前，任何依赖新 unsafe/concurrent-memory primitive 的静态达到候选与性能比较资格 |
| 不受影响的工作 | 当前主仓工程、一般 C、使用既有基元的下界与 outsider 研究、其他静态分量、兼容与恢复工作 |
| 需要谁裁决 | 已完成：项目目标与生产可信基元范围的归属方 |
| 收到的输入与出处 | 用户明确裁决：只有 profiling 证明当前 `BTreeMap` 的 `insert/remove/range` 锁是瓶颈时才重新考虑；现在冻结。`USER_OWNED_DECISION_2026-08-23` |
| 主任务（Primary）核验与吸收结果 | 裁决不修改共识、网络格式、公开 API 或存储格式；不影响当前 R2 工程根；将 probe 保留为构造性证据，不纳入候选、性能或静态达到；重新激活后仍须完成全部 TCB、资源、失败语义和真实性能审计 |
| checkpoint | `00000000000000000258`（其 `MANIFEST.json` 绑定本账本版本） |

### CKB-AUTH-0004：tx-pool 验证时间与执行容量的本地资源语义

#### 需要裁决的问题

CKB-VM 的 cycles 是共识执行成本单位，但它不保证与节点实际验证时间严格成比例。对 tx-pool 来说，真正需要管理的是每次验证占用的墙钟时间，以及有限的排队和执行容量；攻击者声明的 cycles 只是一个不可信输入信号，不能作为真实工作量或安全性的证明。

候选架构计划复用现有有限 worker、compute capability、公平执行许可和资源账本，为每次验证绑定一个固定的本地时间上限；cycles 最多用于选择预先冻结的时间等级，不能购买额外容量，也不能突破无条件硬上限。需要明确：

> 超时结果是否可以成为交易或 peer 的无效/封禁依据，是否可以转播，以及它是否影响区块共识验证？

#### 已核验的实现边界

1. 当前 `verification::TimeRelativeTransactionVerifier::verify_with_pause` 已把可恢复脚本验证接到 `script::resumable_verify_with_signal`；`script/src/verify.rs` 在 VM `run()` 循环中持有 `VMPause`，可以响应中断。
2. 初始 scheduler/ELF 构造发生在 `VMPause` 建立和 VM `run()` 之前，单纯给外层 future 加 `tokio::time::timeout` 不能保证立即停止这段同步加载工作；丢弃 future 也不能替代对子任务的结构化停止与 join。
3. `exec/spawn` 的程序加载发生在 scheduler 消息处理阶段，暂停覆盖仍有边界。项目归属方明确要求：CKB-VM 本体不属于本项目修改范围；ELF loader 对重叠 BSS 的 `zero_ranges` 优化按已落地前提处理。
4. 当前错误代数没有 `ExcessiveVerifyTime`，也没有把 wall-clock 超时纳入共识 cycles 的既有承诺。
5. tx-pool 已有有限 verifier worker、compute capability、远程声明 cycles、公平 semaphore、`ResourceLedger` 的 `active_work/compute_bytes/compute_edges` 以及有界通道；超时必须复用并归还这些现有线性资源，不能另建平行容量账本、失控任务或第二验证队列。

#### 权威裁决

采用以下本地资源语义：

1. `ExcessiveVerifyTime` 是**本节点 tx-pool 的瞬时资源拒绝结果**。本次交易不进入池，也不向其他节点转播。
2. 超时不证明交易违反共识或脚本 cycles 规则。交易若后来出现在区块中，仍走现有区块共识验证路径；wall-clock 上限不得进入区块有效性。
3. 不得因为该结果 ban 交易、ban peer、写永久 invalid/ban cache、形成信誉处罚，或向网络传播“交易共识无效”的结论。
4. 超时必须结构化终止本次可暂停验证，join 对应子任务，归还 worker、compute、资源与调度 capability，然后继续下一笔交易。不得只丢弃外层 future，让 CPU 工作在后台继续。
5. 同一交易以后可以再次按当时节点的本地预算提交验证。抗拒绝服务依靠有限 worker、公平调度、每次尝试的硬时限和现有资源边界，不依靠 ban。
6. binary 预检只能依据确定性的 ELF 事实，在进入不可暂停加载前拒绝超过本地硬工作预算的输入；该拒绝仍是本地资源政策。不得把对动态 `exec/spawn` 的猜测写成永久无效。
7. tx-pool 直接管理的是验证墙钟时间和有限排队/执行容量。声明 cycles 是攻击者可控的分类信号，不是真实资源账本；它最多选择预先冻结的等长或更短时间等级，不能延长无条件硬上限，不能增加 queue/worker 容量，也不能改变拒绝语义。
8. `cycles_per_ms` 若保留，只能作为绑定 VM、脚本版本、CPU 类别和校准身份的本地保守分类参数；不得使用攻击者样本在线扩展后续预算或容量。具体等级、硬上限、抖动容差和配置兼容必须在看结果前冻结并经真实负载校验。
9. 两份 develop 报告只作为“cycles 与实际验证耗时可能分离”的中性必要性证据和回归样本。目标是建立通用 tx-pool 时间/容量机制，不是依靠 tx-pool 关闭报告，也不得为报告、syscall 或 binary 建立特例和 allowlist。

#### 当前处理

| 字段 | 当前值 |
|---|---|
| `request_id` | `CKB-AUTH-0004` |
| `status` | `VERIFIED_AND_ABSORBED` |
| 所属子系统 | `CKB_TX_POOL_VERIFY_TIME_CAPACITY_RESOURCE_AND_LOCAL_REJECTION_SEMANTICS` |
| 影响面 | tx-pool 本地接纳、远程转播、worker 生命周期、错误映射和配置；不修改共识、网络格式、存储格式或区块验证规则 |
| 裁决结果 | tx-pool 直接管理验证墙钟时间和现有有限容量，cycles 仅作不可信分类信号；超时仅作本地瞬时资源拒绝；不入池、不正向转播、不 ban 交易或 peer、不写永久 invalid；区块共识验证不受影响；结构化终止并归还同一个现有 capability 后继续下一笔 |
| CKB-VM 边界 | CKB-VM 本体不在本项目范围；重叠 BSS 的 loader 优化按已落地处理；tx-pool 只负责 wall-clock budget 与确定性 binary 预检 |
| 被阻塞的工作 | `NONE`；具体时间等级、硬上限、校准、预检阈值和错误接线由隔离候选实现与测试决定；不得新建容量账本或报告特例 |
| 权威输入 | 用户以项目目标与设计决策归属方身份明确指定上述本地拒绝、禁止 ban、禁止共识化与不转播边界 |
| 权威出处 | `USER_OWNED_DECISION_2026-08-23_RECORDED_IN_DURABLE_LEDGER` |
| 主任务（Primary）核验与吸收结果 | 已核对当前 pause/loader/worker、`ResourceLedger`、公平执行许可和 sync 负终态投影边界；将该机制列为完整候选在 authority R3 之前必须满足的最高优先级 hostile-input/资源有界硬合同。实现只扩展现有 lease 的时间维度，不另建容量权威；两份报告仅作回归见证，不把 wall-clock 结果升级为共识事实，也不让该局部命题形成长期研究支线 |
| 工程落地 | 主仓 `1d20dda472de3603ccfd71a61192162146c38d6e` 将默认绝对上限冻结为一个 `MIN_BLOCK_INTERVAL`，即 8 秒；配置仍可覆盖。现有 250 ms 下限与每毫秒 10,000 声明 cycles 的宽松估算保留，只作为最终兜底预算，不作为交易质量筛选器。 |
| 校准证据 | `/Users/zhangdingwei/.codex/state/txpool-v8/materials/verify-time-fallback-calibration-r1/MATERIAL_MANIFEST.json`，SHA-256 `4cf6607f1c9b05967b5878fafa004355fdd11db5dac82f4bb515842e720102de`。当前机器上高 cycles 与低 cycles 密度样本均留有明显余量；这不构成所有受支持机器上的普遍时延证明。 |
| 最终边界 | 8 秒是可配置的节点本地最坏占用上限，不是“必须在下一个区块前完成”的协议承诺。正常交易的保护来自宽松绝对上限、可重试语义和不 ban；若受支持的慢机器出现合法交易误伤，只调整本地默认/配置与校准材料，不改变共识或拒绝分类。 |
| checkpoint | `00000000000000000321`（其 `MANIFEST.json` 绑定本账本版本） |

#### 补充裁决：时间预算累计实际 CKB-VM script 验证工作（包含 ELF 装载）

项目归属方进一步明确：tx-pool 的验证时间上限不是“从领取 worker、compute permit 或开始整次尝试起算的绝对墙钟 deadline”，而是**一笔交易在 CKB-VM 中实际完成 script 验证工作的累计墙钟预算；该工作从 root program 数据读取和 ELF 解析开始，而不是从首条 VM 指令开始**。不得因本机排队、调度或 VM 外准备变慢而误杀正常交易。

这项裁决按以下字面合同吸收：

1. `min_tx_verify_time_ms`、`tx_verify_cycles_per_ms` 和 `max_tx_verify_time_ms` 只决定本次交易可使用的累计 VM script-verification-work 时长；声明 cycles 仍只能选择一个不超过硬上限的保守本地预算。
2. 下列时间一律不扣 VM 预算：等待 worker/permit、authority checkout、resolver、snapshot/DB、cache、Tokio 被调度前的等待、contextual 非脚本检查、Suspend 后的 idle、script group 间隙，以及 VM 完成后的 DAO/数据检查。
3. 下列真实验证工作必须扣 VM 预算：root ELF 数据读取、解析、装载与 BSS 初始化，实际 VM 指令执行，以及 script 执行期间由 `exec/spawn` 触发的装载和 scheduler 工作。`InitialProgramLoadLimit` 仍作为独立、确定且有界的字节/映射预检；它不能把实际 loader 耗时从时间账本中删除。本项目不修改共识 cycles 或 CKB-VM 语义。
4. 一笔交易跨全部普通 CKB-VM script group 和多次 Resume 的预算必须累计守恒。每个 group 从真实 root program 准备/装载开始计时，进入和退出 `scheduler.run` 的各段继续累计；返回或 Pause 后停止，Suspend idle 与组间空档不得计时。不可暂停的 loader 小段若越过预算，允许在该同步小段返回后立即形成本地超时，但不得继续执行 VM 或遗留后台任务。
5. 计时权威必须来自实际 VM 验证工作的起止凭证和累计量。父任务的 timer 只负责在可暂停阶段及时 `interrupt + Stop + join`；不能因为 Tokio `select!` 的观察顺序，把预算内已经完成的正常 VM 误判成超时。
6. cache hit、Type ID 内建检查和在 VM child 建立前合法到期的其他本地流程消耗零 VM 时间预算。`InitialLoadExceeded` 是独立的本地装载资源拒绝，不得伪称 `ExcessiveVerifyTime`。
7. 超时后的既有外部边界不变：只作本节点瞬时拒绝，不入池、不正向转播交易、不 ban、不写永久 invalid，不影响区块共识验证；必须结构化终止并 join，归还原有线性 capability。

当前隔离审计已确认旧实现存在偏差：`AuthorityComputeExecutionPermit::started_at` 在取得 semaphore permit 时记录时间，随后生成的绝对 deadline 会错误吞入 resolve、调度、cache/DAO 和 Suspend 空档。该旧计时口径自本裁决起被取代；在 VM-verification-work 累计实现及其反例通过前，候选不得进入最终性能、安全或 Acceptance 裁决。

| 字段 | 当前值 |
|---|---|
| `status` | `VERIFIED_AND_ABSORBED_IMPLEMENTATION_CORRECTION_OPEN` |
| 权威输入 | 用户明确裁决：“timeout 机制必须是 VM script 执行；不能误杀正常交易”；并进一步纠正：ELF load 属于攻击者可放大的 VM 验证工作，必须计入。 |
| 权威出处 | `USER_OWNED_DECISION_2026-08-27` |
| 当前生产偏差 | permit 起点的绝对 wall deadline 会把 VM 外时间计入预算；已由源码链路复现，不是理论风险。 |
| 唯一活根 | `B8_TXPOOL_VM_ACTIVE_EXECUTION_BUDGET_R1` |
| 不改变 | CKB-VM 指令、cycles、script 结果、hardfork、区块共识验证、暂停/停止/join 所有权。 |
| 实现状态 | 隔离设计与实现进行中；主仓尚未同步。 |
| checkpoint | `00000000000000000344`（绑定本补充裁决与当前 OPEN 实现状态） |

### CKB-AUTH-0005：分片候选对独立负载与级联负载的性能取舍

#### 需要裁决的问题

分片 authority 让 support 集合不相交的交易并行提交；关联交易则可能需要按序获取多个 shard。需要明确：若独立交易显著提升，而经过充分优化后的部分级联负载相对 develop 仍有小幅退化，是否必须立即淘汰分片候选。

#### 权威裁决

不因任何可测微退化而轻易淘汰分片候选。它是长期静态审计所得的主要候选，必须按以下规则验收：

1. 一致性、敌对输入安全、兼容、恢复、确定性、资源上限和关闭语义仍是不可交换的硬约束。
2. support 不相交的独立交易必须取得材料性的并发或吞吐提升；噪声范围内改善不能补偿级联退化。
3. 级联负载必须完成最大工程优化，并作为 shard 的正式验收臂。至少覆盖线性 ancestor 链、shared-parent、fan-in/fan-out、RBF 后代闭包和 chain transition；至少评估单 shard 快路径、固定 shard 位图、无分配去重与有序 guard、关联批次摊薄锁成本，以及 profiling 指向的其他直接优化。
4. “最大工程优化”以搜索闭合判定，不按工时或轮次判定：每个主导 profile 成本中心都必须有一个已验证优化，或一个更早硬约束、复杂度或可信核阻断；不得以“仍可继续调优”无限延期，也不得用明显未优化的实现淘汰 shard。
5. 充分优化后，级联负载相对 develop 的有限、预先冻结且可解释退化可以接受。容忍边界必须在查看候选结果前，按负载类别和指标分别写入测量合同；不得看结果后调整。
6. 不得用总体平均吞吐掩盖 p99、CPU、内存、锁等待、恢复或关闭退化。允许的退化必须归因于真实耦合 support 所必需的多 shard 原子协调；多余分配、重复遍历、错误锁范围、观察器或工具包装造成的退化不在容忍范围内。
7. 性能比较必须同时绑定 develop 和进入候选前的当前主分支。develop 证明架构级防御既有反例时仍释放生产性能；当前主分支识别候选相对已有安全改造的真实增量。
8. 第一次超过冻结边界时，先沿真实 profile 热点做有界、源绑定优化。只有没有新的可执行优化 seam，或优化会破坏硬约束、增加第二语义权威、显著扩大可信核时，才冻结或淘汰候选。
9. 不预先引入动态整链共置、组件迁移或第二提交引擎。只有 profiling 证明多 shard guard 是级联主导瓶颈，且更小优化不足时，才允许重新登记这种候选。
10. comparative bench 之前，必须先对隔离 shard 候选做生产同源 profiling，并完成新架构的工程优化。profiling 同时覆盖独立和级联负载；其作用是定位并消除候选热点，不是提前形成候选排名。
11. develop 和进入候选前的当前主分支只作为只读结构参照、归因基线和最终性能基线；不得投入时间为它们另做优化。候选 profiling 达到搜索闭合后，冻结候选源码、二进制、配置和测量合同，随后才运行三方 comparative bench。未优化候选的 bench 不具淘汰或验收效力。

这是 `CKB-AUTH-0002` 所允许的事先冻结项目取舍，不降低字面 `G0`，也不自动关闭 P4、Acceptance、STATIC_ATTAIN 或 G0。

#### 当前处理

| 字段 | 当前值 |
|---|---|
| `request_id` | `CKB-AUTH-0005` |
| `status` | `VERIFIED_AND_ABSORBED` |
| 所属子系统 | `CKB_TX_POOL_SHARDED_AUTHORITY_PERFORMANCE_ACCEPTANCE` |
| 影响面 | 内部生产架构与性能验收；不修改共识、网络格式、公开 API、存储格式或硬资源边界 |
| 裁决结果 | 独立负载须材料性提升；级联负载须完成最大工程优化；先 profiling 并只优化 shard 候选，冻结后再做三方 bench；之后允许相对 develop 的预冻结、有限、逐指标且可解释退化 |
| 双基线 | `develop` 与进入候选前的当前主分支 |
| 被阻塞的工作 | `NONE`；具体容忍阈值在候选测量合同冻结前确定，不等待用户交互 |
| 权威输入 | 用户明确要求：不轻易淘汰长期审计所得的 shard；独立交易提升良好时，应尽力优化级联负载，并允许相对 develop 的有限弱化；级联最大工程优化必须列入 shard 验收 |
| 权威出处 | `USER_OWNED_DECISION_2026-08-23_RECORDED_IN_DURABLE_LEDGER` |
| 主任务（Primary）吸收结果 | 将 shard 验收冻结为“候选生产同源 profiling 与工程优化闭合 → 冻结候选/合同 → develop、当前分支、shard 三方 bench → 独立材料性提升 + 级联预冻结容忍边界”；develop/当前分支只读、不投入优化；禁止平均值掩盖、结果后调阈值、未优化即淘汰、动态共置和第二提交引擎 |
| checkpoint | 待下一不可变 checkpoint 绑定本账本哈希 |

### CKB-AUTH-0006：受已知旧版派生缺陷影响的 `BlockExt` 原地升级承诺

#### 需要裁决的问题

当前候选从原始区块生成的新状态已经通过局部放大、全局贯穿、敌手组合和重启/重组长链路审计；hardfork 规则也由 `ScriptVerificationRules` 固化并进入校验缓存键。本条目不质疑这些结论。

精确未决边界是：历史提交 `585395c9dcfa04607650d3b2c292256c6d498198` 曾可能把受 short-ID cell alias 与 witness-only fee cache 影响的经济派生值写入 `BlockExt.txs_fees`；当前实现会信任已经标记为 verified 的 `BlockExt`，重启或已验证侧链前缀复挂时不会重新做 contextual verification。项目是否承诺从这类**受影响旧版数据库**直接原地升级并继续信任其经济派生值？

#### 已核验边界

1. 这不是 hardfork 切换点错误；规则版本身份已经固化。
2. 隔离 canary 只证明当前消费者会保留注入的旧派生值，没有复现一份生产宽度的旧数据库或具体奖励偏差。
3. 当前候选自己生成的区块、缓存、`BlockExt`、重组和重启状态保持 `UPHELD_SCOPED`。
4. 未得到明确支持矩阵前，不应给所有 `BlockExt` 增加泛化版本字段，也不应全链重验、后台扫描、增加 fallback 或第二经济状态引擎。

#### 可选裁决

| 方案 | 字面含义 | 所需工程动作 |
|---|---|---|
| A | 不承诺从受影响旧版数据库原地升级；只支持从明确不受影响的版本或干净重建状态升级 | 在升级/发布兼容说明中写清最低安全来源版本或重建要求；当前候选无需代码补丁 |
| B | 承诺原地升级 | 先冻结确切受影响版本范围与真实旧数据库工件，再设计一次性、可验证、可恢复的迁移或失效策略；不得用运行时兜底代替迁移 |
| C | 经版本与工件核验，正式支持矩阵中的数据库均不可能包含该派生状态 | 持久化可复现的排除证据；当前候选无需代码补丁 |

#### 当前处理

| 字段 | 当前值 |
|---|---|
| `request_id` | `CKB-AUTH-0006` |
| `status` | `VERIFIED_AND_CLOSED_OUT_OF_TXPOOL_SCOPE` |
| 所属子系统 | `CKB_CHAIN_DATABASE_UPGRADE_COMPATIBILITY_AND_LEGACY_DERIVED_METADATA` |
| 影响面 | 原地升级兼容、历史数据库、奖励派生与发布承诺；不修改 hardfork 规则、当前候选原生状态或网络格式 |
| 默认保守动作 | tx-pool 不修改 `BlockExt`、不增加版本字段、不做全链重验、扫描、fallback 或迁移；继续使用 develop 已有的 chain/database upgrade 对策 |
| 阻塞范围 | `NONE`；不阻塞 tx-pool 重构、有限候选 G0、性能、安全审计或 Acceptance |
| 不阻塞范围 | 本项目全部剩余工作 |
| 需要谁裁决 | 已完成：项目目标与 tx-pool 范围归属方 |
| 收到的输入与出处 | 用户明确裁决：“关闭该条目；它不属于 tx-pool 重构需要考虑的范围，develop 已有相关对策。”`USER_OWNED_DECISION_2026-08-27` |
| 主任务（Primary）核验与吸收结果 | 将旧版数据库派生兼容归还 chain/database upgrade 与发布支持面；不把它误写成当前 tx-pool 候选缺陷，也不在 tx-pool 复制 develop 已有对策。原有审计证据保留，仅作边界记录。 |
| 证据 | `/Users/zhangdingwei/.codex/state/txpool-v8/materials/candidate-long-chain-security-audit-r1/MATERIAL_MANIFEST.json`，SHA-256 `95536bc912e9f6fb764443ae063f28acd1b2a1df847834ab6ba01753c84b6ae0` |
| checkpoint | 待下一不可变 checkpoint 绑定本裁决后的账本哈希 |

### CKB-AUTH-0007：将 G0 的量词域收敛为当前冻结候选集合

#### 需要裁决的问题

原字面 `G0` 要求在整个 `CKB_PRODUCTION_REALIZABLE_ARCHITECTURE_CLASS` 上证明全局最优。一般静态审计已经闭合 `S4`、`S6` 和 `S7` 的非零下界，但 `S1` 的总 support 关系与 `S7` 的可信语义核最小性仍缺少表示无关的全称证明或真实生产 outsider。继续枚举局部操作、包装、翻译器或有限模型不能补足这些量词。

项目归属方现明确裁决：

> canonical `G0` 不再量化开放生产架构类，改为“当前候选集合内最优”。

#### 权威裁决

1. canonical 目标改名并冻结为 `G0_CURRENT_FROZEN_CANDIDATE_SET_R1`。原开放架构类目标保留为研究账 `G0_OPEN_ARCHITECTURE_CLASS_RESEARCH`，状态继续为 `OPEN_FROZEN`，不再阻塞本轮工程交付。
2. 在查看新的 R3 正式结果之前，候选生产集合冻结为：
   - `DEVELOP_BASELINE`：`17d7db5bb423a1b2177e14a132a41d5a91a515f3`；
   - `SHARDED_SELECTED`：生产提交 `d32b357aed21d68ad2f42015c6d3202dd68524e7`，交付主仓提交 `79803fe5a33576179ba49913bd2f48b3f95476a9` 仅额外包含文档符号纠正。
   基准 harness、runner、carrier 提交只承担等价观测，不是第三个生产候选。历史 source-p/q/r/s/t、被否实验和已退休实现不在该集合内。
3. 共识、CKB-VM 指令与 cycles、脚本结果、hardfork 选择、数据完整性、敌对输入安全、公开兼容、恢复、确定性、有限资源和关闭语义仍是不可交换的资格门。任一候选违反即失去资格；不得用性能补偿。
4. 开放架构类的 `STATIC_LOWER`、`STATIC_ATTAIN`、`S1` 与 `S7` 最小性结论保持原状态和证据边界，不得改写为已证明。它们不再是有限候选目标的闭合前提；已有静态反例与硬不变量仍可淘汰有限集合中的候选。
5. 性能裁决只使用结果前冻结的 R3 身份、场景、环境、终态、语料、VM/共识、噪声和失败合同。旧 R2 因观测器非对称干扰仅作诊断，不能排名。总 CPU 保持测量与归因，但没有未经授权的固定比例淘汰门。
6. `SHARDED_SELECTED` 必须完成既定最大工程努力，并满足 `CKB-AUTH-0005`：独立负载取得材料性收益；关联负载经过充分优化；只允许预先批准、有限且可解释的关联退化。develop 中已被冻结的架构级安全反例属于资格证据，不能被吞吐优势抵消。
7. 若冻结集合中的候选仍形成无法由既有权威取舍裁决的 Pareto 交叉，则 `G0_CURRENT_FROZEN_CANDIDATE_SET_R1` 保持 OPEN；不得看完结果再改权重、场景或候选集合。加入新候选必须另建一个结果前冻结的新目标世代，不能暗中修改 R1。
8. R1 的闭合条件是：候选身份与观测合同不可变；工程最大努力有可复现停机证据；相关 correctness、`make integration`、`make test`、fmt/check/clippy 通过；R3 性能证据有效；最终多视角安全审计通过；兼容/恢复的适用边界得到明确处理；复杂度与可维护性复核完成。绿色 package、有限测试或单一吞吐数字均不能单独闭合。

#### 当前处理

| 字段 | 当前值 |
|---|---|
| `request_id` | `CKB-AUTH-0007` |
| `status` | `VERIFIED_AND_ABSORBED_PRE_R3_RESULT` |
| canonical 目标 | `G0_CURRENT_FROZEN_CANDIDATE_SET_R1` |
| 冻结候选集合 | `{ DEVELOP_BASELINE@17d7db5b, SHARDED_SELECTED@d32b357a }` |
| 原开放类目标 | `G0_OPEN_ARCHITECTURE_CLASS_RESEARCH = OPEN_FROZEN` |
| 共识/VM 边界 | exact identity；不得改变 VM 指令、cycles、脚本结果、hardfork 或区块验证语义 |
| 当前工程根 | 修复并冻结无观测干扰的 R3 candidate-vs-develop 测量合同；未查看正式 R3 结果 |
| 不自动关闭 | 一般 `STATIC_LOWER`、`STATIC_ATTAIN`、`S1`、`S7`、安全审计、Acceptance 与有限候选 G0 |
| 权威输入 | 用户明确裁决：“把字面 G0 降级成当前候选集合内最优，否则 G0 无法达到。” |
| 权威出处 | `USER_OWNED_DECISION_2026-08-27_PRE_R3_RESULT` |
| 主任务（Primary）核验与吸收结果 | 接受目标域收敛，但在结果前冻结精确集合、硬门和闭合条件；保留一般静态研究的真实 OPEN 状态；禁止结果后改集合或把测量 carrier 当生产候选。 |
| checkpoint | 待下一不可变 checkpoint 绑定本账本哈希与 R3 修复身份 |

### CKB-AUTH-0008：半迁移分片候选必须完成真实 shard，不得回退单锁

#### 需要裁决的问题

对提交 `eb9b9d9180d3ec1cc8d46614edb819a258d49258` 的全局复审确认：

- owner/resource/index/membership 已具备 64 个物理 shard 和精确 write support；
- 只有窄化的无父子 Accepted 本地删除能在外层 shared barrier 下并发提交；
- 普通 admission、Ready、RBF、dependency 和多数管理 Apply 仍取得
  `AuthorityStoreLock::write`；
- 因而当前实现同时支付全局锁与内层 shard 锁成本，尚未兑现“独立工作最大并行”；
- 默认生产源码约为冻结 develop 的 7.25 倍，过渡层不能永久留在最终 TCB。

需要项目归属方裁决：

> 后续是完成真实 shard，还是回退到单锁并删除分片中间层？

#### 权威裁决

采用“完成真实 shard”，回退单锁不再是允许的后继：

1. `TRUE_SHARD_APPLY_COMPLETION_R1` 是该裁决形成时的唯一工程根（历史快照；当前根见第九节与 `STATE.json`）。
2. 外层 `AuthorityStoreLock` 最终只承担 ordinary commit 的 shared
   chain/generation barrier，以及 chain/generation replacement 的 exclusive
   barrier；不得继续串行普通独立 mutation。
3. 必须改造现有唯一 `PreparedApply / ApplyToken` 引擎；禁止保留 flat/shard
   两套语义引擎。
4. 允许重构 `&mut TxPoolAuthority` 的物理根表示。此前“不许改变该表示”
   不是用户约束，也不是 CKB/Rust 约束。
5. 迁移期间允许下一真实生产 overlap gate 所必需的临时结构；完成该迁移时
   必须删除全局普通写路径、重复 adapter 和无剩余义务的过渡层。
6. scheduler、effect、clock、capacity、chain 等真实共享事实只能保留具名、
   可证的最小 reservation/linearization cut；不能借它们恢复全局长写锁。
7. 第一资格门必须通过真实 retained-ingress → compute → Ready → Plan →
   Apply 路径，让两笔 disjoint ordinary admission 同时进入完整生产 shard
   Apply cut；test-only helper 或 local-removal-only overlap 不算。
8. 点读、全量查询、template、persistence、reorg、effect publication、
   cancellation、stale 和 shutdown 必须保持 coherent、原子、有限且可复现。
9. 迁移完成后重新冻结新的有限候选目标世代并重新测量；旧的 checkpoint 350
   性能只保留为 `eb9` 中间态证据，不能排名完成后的 true-shard 候选。
10. 若工程实现遇到会改变上述设计目标、兼容承诺或硬约束的真实歧义，Primary
    必须主动登记并上报；不得自行冻结义务后切换阶段。

#### 当前处理

| 字段 | 当前值 |
|---|---|
| `request_id` | `CKB-AUTH-0008` |
| `status` | `VERIFIED_AND_ABSORBED` |
| 所属子系统 | `CKB_TX_POOL_TRUE_SHARD_PRODUCTION_COMMIT_ARCHITECTURE` |
| 影响面 | tx-pool 内部提交拓扑、静态独立性、性能与复杂度；不修改共识、VM、wire、公开 API 或持久化格式 |
| 当时唯一根（历史快照） | `TRUE_SHARD_APPLY_COMPLETION_R1`；当前根见第九节与 `STATE.json` |
| 禁止后继 | 回退单锁、保留永久半迁移、第二语义引擎、用 test-only overlap 冒充生产迁移 |
| 允许的迁移 | 重构现有唯一 Apply 根表示；引入有删除期限的必要中间结构；最终收缩过渡 TCB |
| 当前生产基座 | `eb9b9d9180d3ec1cc8d46614edb819a258d49258` / tree `000a3cef8c9d92127f8b0dfb526402ef0d47da58` |
| 控制面纠正提交 | `280b0e00d3e93575c0186cedfa72b29a33ea18f5` |
| 首个隔离实现提交 | `3ed100cdf`，将 EffectLog 迁为 generation-owned 短共享域；全 target check 与 effect-focused 54/54 已通过 |
| 权威输入 | 用户明确要求：“不回滚；真正 shard 必须完成；允许修改旧的自设限制；难题必须主动上报。” |
| 权威出处 | `USER_OWNED_DECISION_2026-08-28` |
| 主任务（Primary）核验与吸收结果 | 撤销 half-shard 的候选/STATIC/max-engineering/Acceptance 身份；仓库合同成为唯一 current-state authority；全部产品 claim 重开；开始改造现有唯一 Apply 引擎。 |
| checkpoint | `351` 已绑定控制提交与 effect/scheduler 迁移；dependency/shared-planner 后继尚未写入该 checkpoint |

### CKB-AUTH-0009：真 shard 下 Accepted effect 的有界预留与可见性切口

#### 已裁决的工程问题

普通 Ready admission 的 owner/index/membership/dependency 可以在精确 shard cut
中无失败提交，但每个成功 admission 还必须产生一个有序、容量受限的 Accepted
effect。若整个 shard commit 一直持有全局 `EffectLog` 锁，两笔 disjoint admission
仍不能同时进入完整生产 cut；若先公开 effect 再提交 owner，则 callback/relay 可能
观察尚未成立的 Accepted；若先提交 owner 再临时申请 effect 容量，则 effect 满或
分配失败会在不可回滚 owner commit 之后发生。

该问题不改变共识、网络协议、公开 API、存储格式或既有用户可见语义，现有硬约束
已经唯一排除全程长锁、提前发布、提交后尽力补 effect 和第二 journal。因此它属于
主任务必须自行闭合的内部工程设计，不再等待项目归属方许可：在现有唯一
`EffectLog` 内加入有界、不可发布的 staged record，由线性 commit capability 在
owner shard commit 完成后将其无失败激活。

#### Primary 已完成的多角度核验

1. `EffectLog` 是全序异步 effects 域，不应重新成为覆盖 owner shard 工作的长锁。
2. 当前 `EffectDelta::Append` 已在 Plan 阶段拥有不可变 batch、Apply sequence、
   class、容量计算和 pending-reject 容量预留；缺的是“已占容量但尚不可发布”的状态。
3. 最小可行协议必须同时满足：
   - staged record 计入原有 effect 容量和队列顺序，但 publisher、callback、relay
     与 recent-reject 查询均不可观察；
   - 在任何 owner mutation 前完成全部 fallible reserve、scheduler/OCC、dependency
     control 与 shard-version 检查；
   - stage capability 未进入 owner mutation 时被丢弃，必须无分配回滚 usage、索引和
     队列记录；
   - owner shard mutation 开始后路径必须无失败，最后 activation 只消费一个线性
     capability，不能返回普通错误；
   - 两个不同 sequence 可先后 stage、并行完成 disjoint shard mutation；publisher
     只能从最老的 committed 头继续，反向完成不能乱序；
   - chain/generation exclusive barrier、shutdown、close、effect settlement 和容量释放
     必须覆盖 staged 生命周期；staged 数量受现有 Ready 并发与 effect batch 上限约束；
   - 完成迁移后不得留下第二 effect engine、轮询修复、watchdog 或无界扫描。
4. “持有 EffectLog 锁覆盖完整 shard commit”安全但重新串行独立 Apply，不满足
   `CKB-AUTH-0008`；“owner 先提交再尽力发 effect”违反既有不可丢 effect 合同；
   “把 Accepted effect 改成可重建提示”会改变 callback/relay 兼容语义，当前不采用。
5. 一个缺少回滚 capability、含 panic 假设的 staged 初稿被自动代码审查拒绝；该补丁
   未写入源码。此负证据确认 staged 协议必须先闭合取消、顺序与无失败激活，而不能
   只加一个布尔标记。

#### 当前处理

| 字段 | 当前值 |
|---|---|
| `request_id` | `CKB-AUTH-0009` |
| `status` | `VERIFIED_AND_ABSORBED_IMPLEMENTED_SCOPED; GLOBAL_TERMINAL_AUDIT_OPEN` |
| 所属子系统 | `CKB_TX_POOL_EFFECT_LINEARIZATION_FOR_TRUE_SHARD_APPLY` |
| 影响面 | tx-pool 内部 effect 容量、callback/relay 可见顺序、shutdown/reorg 生命周期；不修改共识、VM、wire、公开 API 或持久化格式 |
| 源码基座 | 历史设计基座为隔离提交 `8216e2b26`；完整 true-shard 实现经 develop 语义合并与树等价压缩后绑定 `51d282345d1d83119c46cdde8f1115f14561b4ac` |
| 裁决方案 | 单一 EffectLog 内的 bounded staged record + 线性 activation/rollback capability；第一阶段只开放无 victim、无 pending-reject 的纯独立 Accepted，耦合/RBF/驱逐/管理路径继续独占；以负 canary 证明容量竞争、反向完成、提交前取消、close/reorg 和不提前可见 |
| 不接受方案 | 全程持有 effect 长锁、owner 后 best-effort effect、第二 journal、无界补偿扫描、panic/expect 充当提交证明 |
| 实施纪律 | staged 计入原有容量但不可发布；所有可失败工作在 owner mutation 前完成；mutation 后激活无分配、无普通错误；反向完成不得乱序；不得留下第二 effect engine |
| 阻塞范围 | 设计裁决本身已实现；当前阻塞转入全局终审中的 Ready reservation、effect Drop fault visibility 和 chain/dependency composition，而非重新等待权威输入 |
| 不阻塞范围 | 精确 shard support、dependency control/sharded mutation、scheduler OCC、source/read cut、测试与复杂度收缩 |
| 需要谁裁决 | `PRIMARY_ENGINEERING_DECISION_ALREADY_DETERMINED_BY_EXISTING_CONTRACTS` |
| 收到的输入与出处 | `USER_OWNED_DECISION_2026-08-28`：明确批准重分类并按单一 `EffectLog` 的 bounded staged record + 不可复制线性 rollback/activation capability 实施；禁止第二 journal 和任何外部语义改变。 |
| 主任务（Primary）核验与吸收结果 | 单一 `EffectLog` 的 bounded staged record 与线性 activation/rollback 已进入完整 true-shard 主仓并通过迁移期相关性质与聚合门；这只关闭设计与 scoped 实施，不关闭一般 C、全局 TCB 最小性或终审新发现。后续问题按普通工程终审处理，只有真正无法由现有合同裁决的兼容歧义才新增权威条目。 |
| checkpoint | 仓库 handoff commit `fd85bf935dc32146761da246db57a25909ea1834` 与冷检查点 `405` 绑定当前吸收状态；后续文档订正 commit 将取代该 handoff 身份 |

## 五、收到输入后的处理规则

1. 回答只决定“预期行为”，不能替代实现正确性证明。
2. 收到回答后，条目先进入 `INPUT_RECEIVED_UNVERIFIED`；主任务（Primary）必须核对出处、适用版本和影响范围。
3. 若回答是新的设计决策而非既有规范，必须明确记录决策人、日期、适用范围和兼容策略。
4. 吸收时必须列出受影响的生产代码、测试、文档和开放主张，不得暗中削弱已有承诺。
5. 最终状态与账本哈希写入不可变 checkpoint；对话内容本身不作为执行状态。
6. 等待裁决期间，主任务（Primary）继续所有不受阻塞的工作，不等待用户交互。

## 六、新条目模板

| 字段 | 填写要求 |
|---|---|
| `request_id` | `CKB-AUTH-XXXX` |
| `status` | 初始为 `OPEN_NEEDS_AUTHORITY` |
| 所属子系统 | 写明唯一归属模块 |
| 需要裁决的问题 | 一句话即可回答，不能混入多个决策 |
| 影响面 | 从共识、网络协议、公开 API、存储、配置、恢复、运行行为中准确选择 |
| 源码提交 | 完整 Git commit |
| 源码与测试位置 | 精确路径、符号和测试名 |
| 现有证据为什么不足 | 写一个可以被新证据推翻的缺口 |
| 可选方案 | 每个方案都写清外部行为和兼容影响 |
| 默认保守动作 | 等待期间保持的行为 |
| 阻塞范围 | 只写真正被阻塞的决定 |
| 不阻塞范围 | 明确仍可继续的工作 |
| 需要谁裁决 | 维护者、设计负责人、规范或新决策归属方 |
| 收到的输入与出处 | 未收到时写 `PENDING` |
| 主任务（Primary）核验与吸收结果 | 未完成时写 `PENDING` |
| checkpoint | 吸收后的不可变 manifest 哈希 |

## 七、已关闭条目

- `CKB-AUTH-0001`：`VERIFIED_AND_ABSORBED`
- `CKB-AUTH-0002`：`VERIFIED_AND_ABSORBED`
- `CKB-AUTH-0003`：`VERIFIED_AND_ABSORBED`
- `CKB-AUTH-0004`：`VERIFIED_AND_ABSORBED`
- `CKB-AUTH-0005`：`VERIFIED_AND_ABSORBED`
- `CKB-AUTH-0006`：`VERIFIED_AND_CLOSED_OUT_OF_TXPOOL_SCOPE`
- `CKB-AUTH-0007`：`VERIFIED_AND_ABSORBED_PRE_R3_RESULT`
- `CKB-AUTH-0008`：`VERIFIED_AND_ABSORBED`
- `CKB-AUTH-0009`：`VERIFIED_AND_ABSORBED_IMPLEMENTED_SCOPED`

## 八、待裁决条目

- `NONE`

终审发现、性能选择、测试失败、实现根和已经由现有合同唯一决定的修复都不是“待权威裁决”。它们必须由 Primary 按方法论继续复现和裁决，不得转化成等待用户输入。

## 九、当前主仓吸收状态

| 条目 | 当前吸收状态 | 对后继的准确约束 |
|---|---|---|
| `CKB-AUTH-0001` | `ABSORBED` | relay 保持批次/字节/工作/关闭有界；允许同一消费者上的有界批处理与提前排空；`UnknownParents`、`GenerationReset` 保持独立有序边界和最终兜底；不承诺极压逐笔无损。 |
| `CKB-AUTH-0002` | `SUPERSEDED_FOR_CANONICAL_DELIVERY_BY_0007; OPEN_CLASS_RESEARCH_RETAINED` | 原开放生产架构类仍是研究账 `OPEN_FROZEN`；不得把有限候选结论写成开放类全局下界或达到。 |
| `CKB-AUTH-0003` | `ABSORBED_FROZEN` | 未有冻结身份 profiling 将 `BTreeMap insert/remove/range` 锁竞争识别为主导瓶颈；不得重启 `scc/sdd/saa` 或扩大 unsafe/回收 TCB。 |
| `CKB-AUTH-0004` | `ABSORBED_IMPLEMENTATION_PRESENT_FINAL_AUDIT_OPEN` | 验证时间是本地、可重试、不 ban、不共识化的 VM script-work 累计预算；包含 ELF 装载，排除排队和 VM 外空档；CKB-VM 语义不变。当前实现存在于同步主仓，但仍服从最终 correctness/security 审计。 |
| `CKB-AUTH-0005` | `ABSORBED_ACCEPTANCE_PENDING` | true-shard 不能因未榨干的级联微退化轻易淘汰。terminal correctness 闭合前允许用 source-bound、same-binary A/A 和当前 true-shard base-vs-candidate 的因果性能护栏约束架构最小化；这类结果只决定当前候选是否保留必要机制，不构成 develop 排名、`MEASURED_STRONGEST` 或性能 Acceptance。最终独立/关联负载的 develop 比较仍须在 terminal correctness、活性、TCB 与并发证据闭合后，绑定冻结 binary/workload/environment/router-layout/noise 执行。 |
| `CKB-AUTH-0006` | `CLOSED_OUT_OF_TXPOOL_SCOPE` | 旧 `BlockExt` 数据库升级属于 chain/database 发布支持面；tx-pool 不增加版本、扫描、重验或 fallback。 |
| `CKB-AUTH-0007` | `CANONICAL_DELIVERY_GOAL_ACTIVE_IDENTITY_REFRESH_PENDING` | canonical 交付目标是结果前冻结候选集合内最优；开放架构类 G0 保持研究账。完整 true-shard 终审修复后必须在看新结果前冻结新的精确候选 generation，旧 `d32b357a` 集合身份不再用于排名；最终选择还必须最小化 reviewer 理解门槛、change amplification 和后续维护成本。 |
| `CKB-AUTH-0008` | `TRUE_SHARD_ROUTE_MIGRATION_SYNCED_TERMINAL_ACCEPTANCE_OPEN` | 当前原子架构基线 `51d282345` 已实现普通 mutation 零外层写锁回退；终审已发现 Ready、dependency、read-cut、chain/template/ordered-control 阻断候选，因此“迁移同步”不等于“正确性、最小性或 Acceptance 完成”。普通 mutation 禁止任何全局、outer 或换名串行兜底；必须用两个 disjoint production Apply 的 deterministic overlap canary 证明真实并发，并用 writer-intent C2 与可审计锁序证明活性；`parking_lot/deadlock_detection` 只能诊断。 |
| `CKB-AUTH-0009` | `IMPLEMENTED_SCOPED_TERMINAL_COMPOSITION_OPEN` | 单一 `EffectLog` 的 staged record 与线性 activation/rollback 已落地；终审必须继续检查 Ready reservation、Drop fault visibility、反向完成、close/reorg 和 chain composition；禁止第二 journal 或长全局 effect 锁。 |

当前终审 census 可以完整冻结而不作 disposition，先在现有 true-shard 架构内完成由因果性能护栏约束的必要性最小化；不得把该护栏外推为最终 develop 排名。选中并完成一个纵向删除迁移后，以 `make integration` 观察外部行为；若失败，只增加 test-gated、事件驱动且不改变锁序的 causal observability。随后冻结新的简化 identity，重新开启 terminal audit，并至少进行一个不预读旧 findings 的 fresh-eyes round，由 Primary 在独立输出后归并。只有连续两轮均无新 upheld terminal 根、无新 production-bound 失败 canary、无更小根或未付复杂度，且 terminal canary、integration 与 aggregate gates 保持通过，才认定达到边际审计价值边界并进入下一阶段。

长期工程角色由用户明确为：**对 G0 负责的 Primary 工程负责人**。Primary 必须把上述
裁决内化为工作选择、证据裁决、源码集成、子 agent 纠偏、reviewer 成本和持续推进的
责任；repository project state 只保存 cold recovery 所需的最小 live state，不能替代 Primary
判断，也不能把恢复仪式升级成项目目标。控制面本身服从“一个事实一个权威、删除重复
状态、只在物质边界持久化”的同一架构纪律。

当前 identity、phase、root 与下一原子动作只由 `STATE.json` 拥有；当前 round 只由
`AUDIT_PLAN.json` 拥有；候选细节由 `FINDINGS_LEDGER.json` 拥有；稳定执行纪律由
`CONTROL_KERNEL.json` 拥有。账本只拥有用户/项目权威输入，不拥有源码事实或终审结论。
