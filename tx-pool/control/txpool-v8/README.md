# txpool-v8 cold recovery

Read the repository and `tx-pool` `AGENTS.md` files, then run:

```text
python3 -B tx-pool/control/txpool-v8/VERIFY_STATE.py --recover-json
```

`STATE.json` is the only live identity, root and next-action owner.
`CONTROL_KERNEL.json` owns stable engineering discipline;
`FINDINGS_LEDGER.json` owns unresolved findings; `EVIDENCE.json` contains only
receipts reachable from the live action. Read the nine decisions in
`CKB_AUTHORITY_INPUT_LEDGER.md` before changing their constraints.

Regenerate hashes with `VERIFY_STATE.py --write-manifest`. Do not hand-copy
progress, inventories or hashes into another tracked view. Subagents are
optional fresh eyes; the Primary reproduces and adjudicates their results
without waiting on them.
