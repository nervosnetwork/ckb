# Copyable fresh-session resume prompt

Copy the text below into a new AI session whose workspace is
`/Users/zhangdingwei/projects/ckb`:

```text
Take over the active CKB tx-pool v8 true-shard engineering task from the repository-owned handoff. Do not use prior chat history, compact summaries, retired Partner state, or /private/tmp as execution state.

First read /Users/zhangdingwei/projects/ckb/AGENTS.md and /Users/zhangdingwei/projects/ckb/tx-pool/AGENTS.md completely. Then follow /Users/zhangdingwei/projects/ckb/tx-pool/docs/handoff/txpool-v8/README.md in its exact order and run:

python3 /Users/zhangdingwei/projects/ckb/tx-pool/docs/handoff/txpool-v8/VERIFY_HANDOFF.py

If verification fails, do not edit or guess. Restore the exact repository handoff commit or latest content-addressed checkpoint named by the handoff. If it passes, read Section 9 of CKB_AUTHORITY_INPUT_LEDGER.md, load only the source slices named by FINDINGS_LEDGER.json, and continue HANDOFF.json.next_action.

Preserve the canonical finite-candidate delivery goal separately from the open architecture-class research target. The complete ordinary true-shard route migration is synchronized, but global terminal correctness remains OPEN with nine blocking candidates in six root clusters. Continue audit steps 5-6: collect and cluster first, Primary independently reproduce or refute each blocker, write a deterministic no-sleep production-bound canary for each upheld defect, compare the smallest Rust-native root with a strong alternative, and only then implement one self-consistent root slice. Do not patch findings one by one.

Use the method, control and command discipline in OPERATING_SYSTEM.md, METHOD_LEDGER.json, CONTROL_KERNEL.json and AUDIT_PLAN.json exactly. Partner, sub-agent, external report, green tests and generated projections are neutral evidence, never gates. Do not introduce an ordinary global/outer/renamed serial fallback, second policy/dependency/effect engine, second journal, population repair scan, watchdog, result-shaped flag or tool-shaped production refactor.

Run cargo fmt --all to write formatting, make fmt/check/clippy for repository gates, focused cargo nextest run for affected tests, make integration at the integration boundary, and direct make test for the full aggregate. Never use direct cargo test or direct cargo clippy, and never run make quick-test before make test. Do not share one CARGO_TARGET_DIR across different absolute source roots.

Before any compact, interruption or handoff, persist exact commit/tree, verified evidence, blockers and the next action in the repository handoff and create a cold-verifiable checkpoint. Continue autonomously until the canonical delivery goal is genuinely achieved or a new authority decision is strictly required.
```

The prompt is a bootstrap instruction, not a separate current-state authority.
The manifest-bound files it names own the literal state.
