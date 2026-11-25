# Tx-pool guidance

The [root guidance](../AGENTS.md) applies. Refine the existing pool and its affected
callers, preserving the contracts documented below.

Canonical verification decides validity. Pool resource refusal is retryable local
policy, never consensus invalidity or grounds to penalize peers. Determined
consensus failures always take precedence over simultaneous VM CPU-time budget
exhaustion; `ExcessiveVerifyTime` never becomes a persistent rejection or peer
ban, and cached script-proof hits record zero `cpu_budget_ratio`.

Preserve off-lock parallel resolution and canonical VM verification concurrency
(including work sharing read-only cell-deps). Final in-memory state commits
intentionally serialize under `Store`'s single-writer `RwLock<AuthorityState>`
using a strict two-level lock order (`Store.state` strictly above leaf
bookkeeping mutexes `queues`, `outbox.state`, `budget.active`, `budget_cooldown`),
without holding any lock across `.await`, I/O, VM execution, callbacks or payload
drops.

Derive active-job memory budgets and compute permits together from one shared
`ConcurrencyPlan` without host-specific constants, preserving `W + 2` queued
pipeline capacity when `W = 0` or runtime cores are limited and reserving
non-compute runtime headroom before granting extra local compute slots. Keep
blocking publication (callbacks, batched `RecentReject` writes, peer bans, block
updates) isolated on `TxPoolPublisher`; only callback-free, purely in-memory
relay batches may flat-combine inline via `Batch::activate_nonblocking`.

Gap denotes only the two-phase proposal window. Waiting denotes missing
dependencies inside the pool; RPC adapts it to the existing orphan count.
Local RPC submission rejects already-spent dependencies instead of waiting.

Read [architecture](docs/ARCHITECTURE.md) for ownership, commit and lifecycle
contracts, and [maintenance](docs/MAINTENANCE.md) for public behavior and upgrade
compatibility. Preserve each regression's premises and observable outcomes when
simplifying tests. Harness changes use the
[measurement checks](docs/BENCHMARK.md#maintenance-gates); performance claims need
frozen inputs and retained evidence, including failures.
