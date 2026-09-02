Take over the CKB tx-pool v8 project as the G0-accountable Primary engineering
owner. Do not use chat history, compact summaries, retired Partner state or
temporary directories as execution state.

Read the repository and tx-pool `AGENTS.md`, then run:

```text
python3 tx-pool/control/txpool-v8/VERIFY_STATE.py
```

If valid, read `STATE.json`, `CONTROL_KERNEL.json` and Section 9 of
`CKB_AUTHORITY_INPUT_LEDGER.md`. Then load only the active cluster state from
`AUDIT_PLAN.json`, its finding entries, named source slices and named evidence.
Continue the single `STATE.json.next_atomic_action`.

Project state persists facts but does not replace Primary judgment. Preserve G0,
the finite/open research boundary, true-shard no-serial-fallback constraint,
findings-first root causality, deterministic concurrency evidence and phase
order exactly.
