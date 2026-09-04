# Tx-pool reviewer guide

Review the implementation as a small state-transition system, not as a defense
of its history. The target is the necessary state, necessary transitions and
necessary conflicts for CKB semantics. Current names such as authority, shard,
Plan, Apply, Ready and `EffectLog` are incumbent implementation choices, not
preapproved architecture.

## What must be true

Each transaction has one canonical lifecycle location. Resource charge,
dependency/conflict membership, scheduling eligibility, public membership and
template visibility are projections of that fact, not independent truths. A
transition either changes every coupled fact at one atomic visibility point or
changes none. External I/O observes a committed transition and cannot veto it.

Independent transactions must retain end-to-end receive and final-commit
overlap. Independence follows CKB read/write semantics: two transactions that
share only a read-only cell-dep outpoint are independent. A shared write, outer
fallback, population scan or renamed global lock on an ordinary route fails
this requirement even if average throughput looks acceptable.

Hostile bytes, items, dependencies, fanout, closure work, active work, retained
effects and retries remain bounded. Checked-out work is a linear capability:
success, stale input, rejection, cancellation, shutdown and endpoint failure
must each return or settle it exactly once. Consensus, VM, wire, RPC, storage,
configuration, reorg, persistence and shutdown behavior remain compatible
unless an explicit owner decision changes them.

## Read the change in this order

1. Start at ingress or the external command and name its observable result.
2. Follow validation and the immutable evidence captured for planning.
3. List the semantic read set, write set and conflict keys. Unexplained shared
   writes are architecture debt.
4. Locate the final freshness check and atomic visibility point. Ensure no
   allocation, population work, I/O, destruction or `.await` occurs there.
5. Follow post-commit effects, wakes, template sources, relay callbacks and
   every failure/cancellation route.
6. Check the corresponding hostile and concurrency canary through current test
   discovery; a stale hand-written inventory or zero-match filter proves
   nothing.

The shortest production path begins in `src/authority/runtime.rs`. Canonical
transaction state is in `state.rs`; mutation composition is in `plan.rs` and
`plan/`; physical partitioning is in `shard.rs`; dependency, scheduler,
resource and effect mechanisms live in their namesake modules. Public service,
query, template and publisher files show what external observers can see.

## Necessity test for every new concept

For every retained state, type, wrapper, lease, receipt, stage or task, ask:

- Which constructible higher-priority failure becomes possible if it is
  removed?
- Why can Rust ownership, an enum, a private constructor or an existing
  capability not express the same fact directly?
- Does it own a fact, or merely translate the same fact between layers?
- Does it introduce another mutable truth, rollback path, wake, lock edge or
  reviewer jump?
- Is the cost required by CKB semantics, or only by the current topology?

Delete a superseded route and its adapters/tests/docs in the same bounded
slice. Do not add a shadow state machine, retry, watchdog, scan repair or
finding-shaped flag. `develop` is a compatibility and counterfactual oracle,
not the design origin; reuse a smaller safe implementation when it exists, but
do not inherit its known global serialization or split ownership.

## Evidence

Prefer literal production outcomes, algebraic conservation checks and
event-driven production seams. Use a separate model only for a named residual
cross-operation quantifier. Wall-clock sleeps, stress runs, tracing and
`parking_lot` deadlock detection are diagnostics, not ordering proofs. Loom or
Shuttle is justified only after the static lock/wait graph and a deterministic
counterexample isolate a finite interleaving.

Run focused affected tests first. At the owning boundary run:

```text
python3 -B tx-pool/scripts/check_all.py
make fmt
make check
make clippy
make test
make integration
git diff --check
```

Benchmark timing is a separate, source-bound comparison using
[`BENCHMARK.md`](BENCHMARK.md); profiling follows [`PROFILING.md`](../PROFILING.md).
Green tests establish only their named claims. Final acceptance additionally
requires a compact architecture path, preserved true-shard overlap, causal
performance evidence, no unresolved blocking finding and maintainable Rust.
