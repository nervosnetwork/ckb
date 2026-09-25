# Tx-pool guidance

The [root guidance](../AGENTS.md) applies. Refine the existing pool and its affected
callers, preserving the contracts documented below.

Canonical verification decides validity. Pool resource refusal is retryable local
policy, never consensus invalidity or grounds to penalize peers. Preserve ordinary
receive-to-commit concurrency, including work sharing only read-only cell-deps.

Gap denotes only the two-phase proposal window. Waiting denotes missing
dependencies inside the pool; RPC adapts it to the existing orphan count.
Local RPC submission rejects already-spent dependencies instead of waiting.

Read [architecture](docs/ARCHITECTURE.md) for ownership, commit and lifecycle
contracts, and [maintenance](docs/MAINTENANCE.md) for public behavior and upgrade
compatibility. Preserve each regression's premises and observable outcomes when
simplifying tests. Harness changes use the
[measurement checks](docs/BENCHMARK.md#maintenance-gates); performance claims need
frozen inputs and retained evidence, including failures.
