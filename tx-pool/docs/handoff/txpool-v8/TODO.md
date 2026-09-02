# txpool-v8 全局进度面板

本文件是在 Codex 中常驻显示的 repository-owned TodoList。它只投影
`architecture-contract.json.phase_order`、`HANDOFF.json.next_action` 和
`AUDIT_PLAN.json.execution_order`；发生冲突时，这三份机器权威与当前源码优先。
只有源码改变、可复验证据改变下一动作，或 durable claim 裁决完成时才勾选；
轮次、耗时、工具活动、agent 结论和单独绿测试不算进度。

## 当前活动项

- [ ] **C1：为冻结的八个根簇建立 deterministic、no-sleep、production-bound canary，并根因裁决 `RbfConcurrency` 集成失败。**
- [ ] 当前阶段仍是 `terminal_correctness_and_root_repair`；生产语义尚未修改。

## A. 接管与控制面

- [x] 完整读取根与 tx-pool `AGENTS.md`。
- [x] 按 handoff README 完成 manifest-bound 首读。
- [x] 运行 `VERIFY_HANDOFF.py` 并确认接管基线有效。
- [x] 读取 authority ledger Section 9 与命名的 `CKB-AUTH-0007` G0 决策。
- [x] 建立当前源码身份的 Cargo、符号、调用与 same-class 索引。
- [x] 吸收新增的标杆资格、true-shard 无串行兜底、锁/并发可测可控和 reviewer 成本约束。
- [x] 完成报告裁决物质边界后的面板与 manifest-bound handoff 更新。

## B. Findings-first 核验与归并

- [x] 原九项候选、六个 blocker cluster 完成 Primary 源码因果链首轮复现。
- [x] 完成三组正交 fresh-eyes 根设计与 strongest-counterexplanation 输入。
- [x] 完整读取用户中性报告。
- [x] focused 动态证据确认 external-effect timeout 可在 callback 未终止时 drain 并判 persistence eligible。
- [x] 逐实例给出 `UPHELD / REFUTED / CONDITIONAL_ASSURANCE / DEFER_LATER_PHASE`。
- [x] 每项记录 source/control/sink、生产可达条件、最强反解释与 proof gap。
- [x] 与原候选去重；同一 authority fact、linearization point、observation 只保留一个根簇。
- [x] 冻结八个 blocker cluster、十一个候选的 census；same-class source surface 可扩展但不按 finding 计数。

## C. Deterministic production-bound canaries

- [ ] Ready malformed head 在 final cut 前遇到更强 Ready。
- [ ] Dependency reverse completion 使用 actual committed observation。
- [ ] C2：`read_all` + writer intent + production nested routed read，无 wall-clock sleep。
- [ ] Lock acquisition edge audit 与全局 DAG 检查。
- [ ] Exclusive policy spender/owner torn-read interposition。
- [ ] Chain receipt 后新增 conflict/dependency reader interposition。
- [ ] Template source check 后 chain change，以及 delayed notification。
- [ ] Clear S0 / Reconcile S1 / late clear。
- [ ] Ordered shutdown seal、close admission、drain admitted/in-flight Reconcile。
- [ ] Full query/template/persistence 固定分页与 final coherent validation。
- [ ] External effect timeout 后不得跨 generation/shutdown 失去 owner。
- [ ] Post-owner ordered projection allocation 必须在第一笔 owner write 前拥有无失败 capability。
- [ ] Ready slot poison 若 upheld，必须由 owning terminal 现场裁决。
- [ ] 两个 disjoint ordinary production Apply 必须真实同时 overlap。
- [ ] 普通 global/renamed serial fallback 的结构负 canary。

## P. 并行 Draft PR 轨（不改变审计 phase order）

- [x] Fetch 并冻结 `origin/develop@17d7db5bb423a1b2177e14a132a41d5a91a515f3`。
- [x] 裁定压缩基线与保留旧分支作为 archive。
- [x] 创建并切换 `zhangsoledad/txpool-true-shard-authority`。
- [x] 在新分支语义 merge develop；已覆盖取当前，否则吸收 develop，禁止全局 `-X ours`。
- [x] 核验 legacy owner 未复活，relay/raw-hash/cache/stale-parent 生产语义未被回流破坏。
- [x] 把最终 merge tree 压为一个原子架构 commit，并证明 handoff 之外逐路径 tree 等价。
- [x] merge identity 的 focused、fmt/check/clippy 通过。
- [ ] SemVer/API owning release decision、`make integration`、`make test` 与 checker；integration 先由 C2 canary 根因并修复，禁止重跑到绿。
- [ ] Push 新分支并创建明确标注 OPEN terminal work 的 Draft PR。

## D. 高视角根设计裁决

- [ ] 每个最终根簇比较最小 Rust-native 根与至少一个强替代。
- [ ] 记录 authority cuts、非法状态、锁、分配、任务、通道、TCB、兼容、恢复与 reviewer cognition cost。
- [ ] 证明每个根覆盖 same-class surfaces，不留下 finding-shaped route。
- [ ] 明确 rare chain/generation barrier 与 ordinary true-shard mutation 的边界。

## R. Fresh-eyes 终审循环

- [ ] 当前八根簇全部由 deterministic canary 裁决并完成根修复后，运行 `make integration`；绿灯不替代 canary。
- [ ] 若 integration 仍失败，只增加 test-gated、事件驱动、不改变锁序的生产绑定观测，再归并根因。
- [ ] 冻结新 identity；至少一个独立视角先不读旧 findings，完成 blind fresh-eyes delta audit。
- [ ] Primary 在独立输出后归并；新 surface 只按 authority fact、linearization point、observation 计根。
- [ ] 连续两轮无新 upheld terminal 根、无新失败 canary、无更小根或未付复杂度，且 gates 保持绿，才认定达到边际审计价值边界。
- [ ] 达到边界后退出 terminal correctness，进入下一阶段；performance/final security/Acceptance 不在本轮偷跑。

## E. 根切片实施

- [ ] Held-cut / policy-reader coherence 与锁活性根。
- [ ] Chain receipt + template publication/notification source fence 根。
- [ ] Dependency actual-commit observation receipt 根。
- [ ] Ordered control seal/drain + ClearCurrent intent 根。
- [ ] Ready reservation/slot-claim conservation 根。
- [ ] External-effect fixed ownership/terminal-knowledge 根。
- [ ] Query/template/persistence population capture 根。
- [ ] 每个切片删除被替代 route、wrapper、模型、注释与 checker 假设，并通过 focused gate。

## F. Correctness identity 门禁

- [ ] 新 canary 先证明当前反例，再在 root repair 后转绿；selector 必须非零。
- [ ] Affected-package focused `cargo nextest run`。
- [ ] `cargo fmt --all`。
- [ ] `make fmt`。
- [ ] `make check`。
- [ ] `make clippy`。
- [ ] `make integration`。
- [ ] 直接 `make test`，不得先跑 `make quick-test`。
- [ ] `python3 -B tx-pool/scripts/check_all.py`。
- [ ] `git diff --check`。
- [ ] architecture/review/validation/handoff/manifest 绑定同一最终 identity。

## G. Correctness 之后的最终目标

- [ ] Hard/static proof。
- [ ] 在看结果前冻结新的 `G0_CURRENT_FROZEN_CANDIDATE_SET_NEXT_GENERATION`。
- [ ] 与 develop 做独立和关联负载的 frozen performance comparison。
- [ ] TCB、LoC、types、locks、tasks、channels、caches 与 change amplification 最小性复核。
- [ ] Final adversarial security。
- [ ] Cold reviewer acceptance：证据优越、复杂度收敛、理解门槛与维护成本下降。
- [ ] Product acceptance 与 final goal closure。

## 禁止提前项

- [ ] 在 terminal correctness 未闭合前，不推进 performance、final security 或 Acceptance。
- [ ] 不用全局/换名串行兜底、第二语义引擎、扫描修复、retry/watchdog 或 finding-shaped flag 关闭问题。
- [ ] 不把 agent、报告、绿测试、文档自述或本 TodoList 当作 claim closure。
