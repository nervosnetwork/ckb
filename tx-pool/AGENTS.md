# Tx-Pool Instructions

These instructions apply to every task under `tx-pool/`. Repository-wide
safety, Rust, validation and handoff rules remain inherited from the root
`AGENTS.md`.

## Stable subsystem boundaries

- Treat the implemented CKB consensus checks and declared compatibility
  surfaces as factual authorities. RFC prose is explanatory and cannot replace
  an end-to-end observation of the implemented protocol.
- Trace a behavior change through ingress, validation, dependency/conflict
  ownership, the atomic authority cut, committed effects, public reads,
  proposal/template use and shutdown. A cache, index, queue or test model may
  not become transaction or policy authority.
- Changes to proposal history or status must preserve the real
  `ProposalView` to `TwoPhaseCommitVerifier` observations for main-chain,
  uncle, genesis and reorg histories. Consensus verification must remain
  independent of the rebuildable tx-pool projection.
- Preserve exact transaction ownership and bounded resource, edge, work and
  capability conservation across every ordinary, stale, cancelled, pressure,
  reorg and shutdown outcome.

## Validation

- Prefer compiler-bound production property and differential tests over
  hand-maintained symbol or behavior inventories. Use a separate mathematical
  model only for a named cross-operation safety, composition or liveness claim.
- Treat the feature-gated profiling instrumentation, capture/analyze runner,
  one-shot workload harness and benchmark runners as maintained development
  components shipped with the production source, not disposable experiments.
  A production ingress, lifecycle, authority or observation change updates the
  applicable profiling and benchmark path in the same owned slice.
- Profiling remains semantically read-only, bounded and disabled by default.
  Its reproducible artifacts bind source, binary, workload, environment,
  target window, raw samples and deterministic analysis; they prove causal
  cost attribution only for that identity and never replace correctness or
  global optimality proof. `PROFILING.md` owns its maintained entry points,
  coverage, evidence boundary and analyzer canaries.
- Before an expensive build, test universe, model search, mutation run, formal
  run or benchmark, preflight the exact subject, tool and command, discovered
  scope, resource isolation, expected discriminator, abort rule and output
  destination. A preflight may stop or replan work; it is never proof evidence.
- Every audit or review combines a locally magnified end-to-end causal slice
  with a global propagation scan across producers, consumers, observations,
  failure/resource paths and same-class surfaces. Either view alone is partial.
- Run focused affected package tests first, then the repository gates required
  by the root instructions at the owning boundary. Cross-crate protocol changes
  include the proposal-table and contextual-verifier suites.
