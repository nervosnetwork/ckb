# txpool-v8 冷恢复入口

本目录保存对 G0 负责的 Primary 工程负责人的最小可恢复状态。`STATE.json` 只持久化
identity、phase、当前 root 和一个下一原子动作；它不替 Primary 选择根或裁决证据。

## 启动

1. 完整读取仓库根 `AGENTS.md` 与 `tx-pool/AGENTS.md`。
2. 运行：

   ```text
   python3 tx-pool/control/txpool-v8/VERIFY_STATE.py
   ```

3. 验证通过后完整读取 `STATE.json`、`CONTROL_KERNEL.json`，再读
   `CKB_AUTHORITY_INPUT_LEDGER.md` Section 9。
4. 只加载 `AUDIT_PLAN.json` 中 active cluster 的状态、该 cluster 在
   `FINDINGS_LEDGER.json` 中的条目、命名源码切片和命名证据。
5. 从唯一 `next_atomic_action` 继续；不要重建聊天历史或重扫未变化源码。

`CONTEXT_LOAD_POLICY.json` 定义 normal Primary 与 blind fresh-eyes 两条读取路径。
`TODO.md` 只是 Codex 可见投影。`EVIDENCE.json`、用户报告、历史 agent 结果、
方法账本和文档审计都按需读取，不是 live state。

验证失败时停止修改并恢复精确 Git cut。禁止用聊天、compact 摘要、退休 Partner、
未引用 checkpoint 或 `/private/tmp` 猜测状态。
