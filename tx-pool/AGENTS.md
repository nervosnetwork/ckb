# Tx-pool instructions

The root `AGENTS.md` owns general engineering and delivery rules. This file adds
pool contracts and current scope. Start with [architecture](docs/ARCHITECTURE.md)
and follow the relevant [review](docs/REVIEW_GUIDE.md) and
[maintenance](docs/MAINTENANCE.md) paths for the change.

## Semantic boundaries

- Preserve declared consensus, VM, wire, API, storage, configuration, reorg,
  recovery and shutdown contracts. `arrival_time`/`fee_rate` controls selection,
  not completion order. Proposal projection must agree with `ProposalView` and
  `TwoPhaseCommitVerifier` for main-chain, uncle and genesis histories; consensus
  verification remains independent of pool projection.
- Keep one truth per semantic fact. Derived indexes and caches need explicit
  sources, validity and bounds. Validate the original premises of decisions and
  rejections; commit coupled owner changes and required effect obligations atomically.
  Rejection exposes no partial state. Publish after commit in the required order,
  retaining obligations through waiting and failure.
- Derive conflicts from transaction semantics. Ordinary independent work has no
  global serial fallback; sharing only a read-only cell-dep stays compatible.
  Preserve receive-to-commit overlap on production paths. Authority locks must
  not span await or foreign calls.
- Bound retained and transient bytes, items, work, edges, fanout and tasks through
  their actual producers and transfers. Include shared backing, container capacity,
  source/destination overlap and concurrent allocations; a final owner check cannot
  bound earlier allocations. Track ownership and charges through rejection, stale
  work, pressure, cancellation, reorg, clear and shutdown.
- Block priority covers all pool computation lanes and direct local requests.
  Cooperative VM suspension retains the transaction, execution state and resource
  charges; resume continues that execution. Reconciliation and publication must
  remain live, and the block consumer must signal resume as its backlog empties.
  Stop remains terminal; a pause request alone does not establish VM quiescence.
- Local VM-time policy includes ELF loading and excludes queueing and suspended
  intervals. Resource refusal is retryable local policy, never consensus invalidity
  or grounds to ban a peer. Pool budgets do not imply a whole-process RSS bound.

## Changes and evidence

- Keep ownership, ordering, errors and bounds visible at responsible interfaces.
  Implement within the current pool and retire superseded routes; do not introduce
  a second pool or long-lived shadow machine. Follow callers and consumers outside
  this directory when their contract changes.
- Use the [review guide](docs/REVIEW_GUIDE.md) to select behavioral regressions.
  Follow [measurement gates](docs/BENCHMARK.md#maintenance-gates) for harness or
  script changes. Performance claims require measurements on frozen inputs;
  preserve their packets and failed attempts. Finite comparisons do not establish
  global optimality.

## Current refactor scope and continuity

The current refactor excludes CKB-VM changes, including dependency patches,
forks and vendoring. Retain its approximately 15k production-line scope: necessary
additions need a behavioral reason and comparison with simpler existing code.
Do not relocate, compress or hide logic to meet the count; counts alone prove
neither necessity nor completeness. These are current task constraints, not new
permanent VM or line-count policies. Changing methods does not relax user constraints.

When present, the project-local state lives under the path returned by
`git rev-parse --path-format=absolute --git-path txpool-v8`.
Read `STATE.json` and the needed evidence before resuming recorded work; preserve
ongoing objectives and authority decisions, and verify source and live process
identities before restarting commands. This metadata is not a CI prerequisite;
current design and maintenance knowledge must remain in the delivered docs.
