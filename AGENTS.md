# CKB Repository Instructions

These are the stable defaults Codex should apply to every task in this
repository. Put subsystem architecture, long-running plans, live findings and
status in a closer `AGENTS.md` or their owning machine artifact.

## Navigate and preserve the workspace

- Before editing, inspect the applicable instruction files, branch, HEAD,
  index and worktree. Existing changes belong to the user unless proven
  otherwise; preserve unrelated work and never use destructive Git commands.
- Use `rg`/`rg --files` for discovery and `apply_patch` for reviewable semantic
  edits. For large Rust work, derive the Cargo graph and a symbol/reference
  index once per source identity, then inspect exact slices instead of repeatedly
  scanning whole files.
- This file grants no permission to mutate external systems, publish, merge or
  release. Do not push unless the user explicitly asks.
- A closer `AGENTS.md` adds subtree guidance. Use `AGENTS.override.md` only for
  a deliberate temporary override, and remove it when the override ends.

## Engineering priorities

Resolve tradeoffs in this order: consensus/data integrity; hostile-input
security; static guarantees, ownership and determinism; declared compatibility
and recovery; bounded resource use, performance and independent concurrency;
maintainability; convenience and source size. Never weaken an earlier item for
a later one without an explicit owned decision.

Green tests are evidence, not the objective. Reproduce failures and trace the
owning producer, consumer and externally visible observation before changing
production code. Fix the owning design once; do not add finding-shaped flags,
retries, timers, scans, watchdogs, fallbacks or allowlists.

## Rust design rules

- Make illegal states unrepresentable with enums, newtypes, private or sealed
  constructors, exhaustive matches and linear capabilities. Keep one authority
  for each fact and one lifecycle location for each object.
- For stateful code, separate `validate -> plan -> apply -> effects`.
  Validation is non-mutating; planning reads one coherent cut and performs no
  I/O; Apply revalidates freshness and commits the smallest atomic change;
  effects run after authority release and cannot veto the commit.
- Derived caches and indexes own no policy and must declare identity, validity,
  rebuild and resource bounds. Accounting, indexes, capabilities and
  publication change atomically.
- Use checked arithmetic at trust boundaries and domain `Result`/`Option` in
  core code. Production panic/assert/unwrap/expect/catch-unwind needs a local
  proof of unreachability and intended termination.
- Never hold a lock across `await`. Authority critical sections avoid I/O,
  attacker-sized allocation, clone, destruction and population scans. Every
  task has an owner, cancellation, join/shutdown and capability-return path.
  Bound hostile bytes, counts, edges, fanout, depth, retries, allocation and
  work.
- Preserve consensus, wire, public API, storage, configuration and operational
  compatibility unless the owning contract records a total decision.

## Build and validation

- Use repository commands from `Makefile`: `make check` for the all-target
  compile check, `make clippy` for linting, `make fmt` for formatting, and
  `make quick-test`/`make test` for the declared Nextest universes. Prefer
  focused affected crate/tests first; run full-workspace gates at the owning
  boundary.
- Serialize commands that compile the same Cargo graph and reuse their
  artifacts. Parallelize read-only checks or isolated test shards; give
  integration shards separate processes, ports and data directories.
- Format changed Rust files/packages and run `git diff --check`. Treat warnings
  as owner defects; do not hide them with filtering or blanket allowances.
- A test, model or checker must name the claim it proves. Filtered, ignored,
  commented-out, undiscovered or unstarted tests do not prove their universe.
  Performance evidence binds binary, workload, environment, runner, causal
  prediction and noise rule.

## Change and handoff discipline

- Retire a route in one bounded migration slice: remove its exclusive code,
  artifacts, evidence, checker/document references and live projections. If an
  external dependency prevents removal, retain one owned blocker with an exit
  condition; an obsolete label is not retirement.
- Update architecture, behavior, validation and public/release surfaces together
  when the change affects them. Stage only intended files when staging is
  requested.
- Before handoff or compaction, persist exact identities, verified results,
  blockers and the next action in the owning project artifacts. Conversation,
  timestamps and prose summaries are not execution state.
- Completion requires the requested implementation, relevant focused and
  aggregate gates, a clean diff check, preserved user changes, and precise
  disclosure of remaining blockers or unrun gates.
