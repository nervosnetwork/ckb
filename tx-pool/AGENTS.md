# Tx-pool guidance

The [root guidance](../AGENTS.md) applies. Refine the existing pool and its affected
callers, preserving the contracts documented below.

Canonical verification decides validity. Pool resource refusal is retryable local
policy, never consensus invalidity or grounds to penalize peers. Preserve ordinary
receive-to-commit concurrency, including work sharing only read-only cell-deps.

Gap denotes only the two-phase proposal window. Waiting denotes missing
dependencies inside the pool; RPC adapts it to the existing orphan count.
Local RPC submission rejects already-spent dependencies instead of waiting.

Use the reference that matches the change:

- Owner model, admission, replacement or conflicts: [architecture](docs/ARCHITECTURE.md)
  and [commit contracts](docs/architecture/COMMIT.md), including original-premise
  validation, atomic owner/effect changes and lock ordering.
- Scheduling, VM budgets, retention, reorg, recovery, publication or shutdown:
  [execution contracts](docs/architecture/EXECUTION.md), including allocation and
  transfer bounds, cooperative block priority and owned cleanup.
- Public callers, configuration or stored data: [maintenance](docs/MAINTENANCE.md)
  for completion guarantees and existing-node upgrade compatibility.
- Behavioral regression coverage: [review guide](docs/REVIEW_GUIDE.md). Preserve
  the scenario's premises and observable outcomes when simplifying tests.
- Harness or script changes: [measurement gates](docs/BENCHMARK.md#maintenance-gates).
  Performance claims need frozen inputs and retained evidence, including failures.
