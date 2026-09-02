# txpool-v8 新会话交接入口

本目录是当前 tx-pool 长任务的仓库内单一交接事实源。它面向任意具备代码执行能力的 AI，不假设接手者是 Codex，也不依赖聊天历史、compact 摘要、外部 Partner 通道或 `/private/tmp` 存活。

接手者必须按以下顺序执行：

1. 完整读取仓库根 `AGENTS.md` 和 `tx-pool/AGENTS.md`。
2. 依次读取本目录的 `MANIFEST.json`、`HANDOFF.json`、`CONTROL_KERNEL.json`、`METHOD_LEDGER.json`、`OPERATING_SYSTEM.md`、`DOCUMENT_AUTHORITY.json`、`DOCUMENT_AUDIT.json`、`AUDIT_PLAN.json`、`TODO.md`、`FINDINGS_LEDGER.json`、`USER_REPORT_VALIDATION.json`、`EVIDENCE.json`、`SUBAGENT_RESULTS.json`、`CONTEXT_LOAD_POLICY.json` 和 `VERIFY_HANDOFF.py`。
3. 运行 `python3 tx-pool/docs/handoff/txpool-v8/VERIFY_HANDOFF.py`。验证失败时停止修改，先恢复本目录绑定的精确源码身份。
4. 验证后读取 `CKB_AUTHORITY_INPUT_LEDGER.md` 的 Section 9；只有命名决策需要时才读取对应历史章节。不要用旧章节的“当前处理”覆盖 Section 9。
5. 不回读旧聊天、compact 历史或未被本目录哈希引用的旧 checkpoint；这些不是执行状态。
6. 从 `HANDOFF.json.next_action` 开始，继续“先汇总、归并、Primary 独立复现，再根因修复”的终审，不把任何 agent、外部意见、绿测试或文档自述当门禁或闭合证明。

`TODO.md` 是供 Codex 常驻显示的人类进度投影，不是第二计划。它必须与
`HANDOFF.json.next_action`、`AUDIT_PLAN.json.execution_order` 和
`architecture-contract.json.phase_order` 保持一致，冲突时机器权威优先。

当前主仓已经包含完整 true-shard 迁移，但终审尚未通过。禁止把“已同步主仓”误读为“已验收”。

需要开启新会话时，直接复制 [`RESUME_PROMPT.md`](RESUME_PROMPT.md) 中的
文本；它只负责引导接手者进入本目录，不另建状态。

`DOCUMENT_AUTHORITY.json` 定义每份文档的唯一角色，`DOCUMENT_AUDIT.json`
记录本次全量整理和一致性检查。接手者不得用稳定合同、历史证据、生成
投影、测试夹具或源码注释覆盖本 handoff 的当前状态；发生冲突时，以实现
的协议、当前源码和本目录的 manifest-bound 状态为准。
