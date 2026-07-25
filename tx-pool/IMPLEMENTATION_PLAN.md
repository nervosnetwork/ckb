# Tx-Pool Root-Cause Redesign — Execution Plan

Design authority: [`ARCHITECTURE.md`](ARCHITECTURE.md)
Independent audit: [`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md)  
Review evidence: [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md)

Status: plan frozen for execution. Correctness and static safety precede the
final separately measured performance gate. Each phase is a recoverable commit
and ends with a whole-architecture review, not only a review of edited files.

## 1. Execution rules

1. The target model has exactly two executable authorities and the seven states
   frozen in the design. A phase may remove old encoding; it may not introduce
   a transitional payload owner, transferable queue or compensating state.
2. Work proceeds deletion-first. A production phase is net-negative in lines
   and removes the named old mechanisms in the same checkpoint that cuts over
   their last caller.
3. A normal implementation defect is fixed only by completing the already
   frozen rule. If evidence contradicts the rule itself, revert the phase and
   reopen architecture audit; do not add local undo/state/index layers.
4. Tests are behavior evidence. A stale test is changed only with an explicit
   policy-register and review-guide update. A product defect is fixed at its
   authority/Plan boundary, not in the integration adapter.
5. Tests use `cargo nextest`; process integration uses `make integration` with
   the complete generated tx-pool impact universe, not only
   `test/src/specs/tx_pool`.
6. No benchmark runs before P7. P7 compares the saved pre-redesign checkpoint,
   `develop`, and final code with unchanged workloads/safety assertions.
7. Each passing phase is committed. A failing hard gate is reverted to the
   previous checkpoint rather than patched forward with new architecture.

## 2. Recoverable checkpoints

| Checkpoint | Content | Recovery meaning |
|---|---|---|
| C0 | `35cabc9b7` | committed current coordinator baseline |
| C1 | preserved correctness/integration fixes + audited design/plan | pre-redesign A/B and rollback base |
| C2 | P0 formal contract/model/evidence migration | documentation and oracle base |
| C3 | P1 PrePoolKernel vertical cutover | old coordinator/runtime/conflict owner gone |
| C4 | P2 PoolMutationPlan/causal graph cutover | nested undo and post-removal decisions gone |
| C5 | P3 stable effect shell cutover | dynamic reservation/fail-stop effect protocol gone |
| C6 | P4 chain/wait/persistence/DefectDomain convergence | recovery lock and service fail-stop gone |
| C7 | P5 correctness/review acceptance | unit/static/document evidence complete |
| C8 | P6 integration acceptance | complete process behavior green/classified |
| C9 | P7 performance/production acceptance | A/B green and final audit complete |

No checkpoint claims a later gate. In particular C1-C8 are not performance
verdicts.

## 3. P0 — Formal contract and migration matrix

### Work

- Promote the audited design to the permanent architecture authority and link
  it bidirectionally with `pipeline.md`, `REVIEW_GUIDE.md`, the security ledger,
  behavior registry, manifest and inventory.
- Freeze machine-readable target authorities, states, commands, ReadyKey,
  PlanOutcome, resource classes/equations, lock edges, displacement causes,
  effect regions, policy differences and residual risks.
- Add a test-only small recomputing reference model. Its indexes are derived
  from primary maps on every command; it must not share optimized production
  transition code.
- Give all 152 ledger findings at least one F1-F8 family and target behavior
  mapping. Validators reject missing mappings and stale test/source anchors.
- Regenerate the integration impact universe from all specs that exercise
  submit/test/clear/remove/pool-info/detail RPCs, relay transaction/proposal
  paths, RBF, persistence/restart, reorg, mining/proposal/template, compact
  blocks and fee estimation. Record why every included spec is relevant and why
  any syntactic candidate is excluded.
- Record production/test/benchmark line counts separately and the exact guard
  count command.

### Exit gates

- documentation/manifest/inventory validators pass;
- the reference model enumerates every state/command outcome and stale lease;
- every retained mechanism has one develop/current counterexample, one target
  invariant and one falsification behavior;
- no production behavior changes and no benchmark starts.

### Whole-architecture review

Re-run authority/state/lock/resource/policy/performance-feasibility matrices.
If the evidence schema needs a new production concept, P0 fails—the schema must
describe the frozen model, not design it.

## 4. P1 — PrePoolKernel vertical cutover

### Work

- Replace `PipelineCoordinator`/`PipelineRuntime` in place with a concrete,
  non-generic stable `PrePoolKernel` shell and swappable entry generation.
- Implement the seven closed variants, global checked `u128` version, exact
  full-hash owner map, unique collision-aware short-ID index, canonical compact
  state vectors and capacity-charged containers.
- Implement Remote/Proposal partitions, global/per-peer limits, exact
  per-owner fair runnable heads, ingress/work permits and residence deadlines.
  Local remains a direct charged borrower and is never a retained source.
- Implement checkout/complete leases, exact `ReadyKey`, global/per-input Ready
  indexes, one serial commit driver and final version/rank checks. Do not add a
  `Committing` state.
- Merge Missing and historical Conflict ownership into Wait; implement atomic
  park, ordered epoch cursor, pending follow-up and resolved-dependency reverse
  projection. Resolve completion is `TxPool read -> kernel` before the read
  opens.
- Rewire workers, RPC/query and administrative removal directly. Delete the
  old coordinator generic bundles, dual tokens, raw stages, relation counts,
  ticket heaps, capacity victim transactions, coordinator undo/audit runtime,
  `ConflictCache` payload ownership and RaceLost/Invalidated/Committing.
- Retain the existing accepted PoolCommitJournal and effect implementation only
  as explicitly tracked dependencies of P2/P3; do not emulate deleted pre-pool
  states around them.

### Exit gates

- phase production lines are net-negative;
- repository search finds exactly two executable transaction owners and none of
  the deleted pre-pool mechanism names in production;
- model differential covers ownership, ABA, promotion, Local cancellation,
  per-peer saturation, dirty-key mutation, worker panic/cancel and stale
  completion;
- targeted nextest and admission/relay/orphan/RBF process specs pass.

### Whole-architecture review

Recompute the ownership/resource equations from actual fields and allocated
capacities; inspect every TxPool→kernel edge; compare policy with section 22.4;
verify the remaining old pool/effect code is only the declared C3→C4/C5
migration debt.

## 5. P2 — PoolMutationPlan and causal graph cutover

### Work

- Make accepted primary identity full-hash based with a unique
  collision-detecting proposal-slot index. Make links/status/sort/outpoint/
  aggregate totals rebuildable projections.
- Implement the explicit displacement authority table and one immutable
  `PoolMutationPlan`: RBF union, role-aware final resolution, causal ancestor
  limits, exact sparse virtual serialized/resident eviction, retained conflict
  subset, effects and prepared projection delta.
- Implement total Apply with checked arithmetic and pre-reserved container
  capacity. Every non-Apply outcome leaves byte-for-byte equivalent primary and
  recomputed views.
- Split resolver roles: inputs observe pool spends; cell deps/dep groups read
  pre-spend chain/accepted producer data and ignore pool consumers.
- Restrict persistent links/weights/cascades to causal producers. Add bounded
  selected-set conditional edges, deterministic topological order and cycle
  shedding in `TxSelector`.
- Pair the final kernel CAS/delta with pool Apply. Delete PoolCommitJournal,
  restore-before-recover, cell-ref escape eviction, nested/cohort undo and every
  fallible post-removal handoff.

### Exit gates

- no production `undo`, rollback journal or fallible ordinary Apply remains;
- exhaustive small-model and randomized differential match current CPFP/status
  eviction policy except the documented cell-ref policy;
- failures injected after every Plan step leave primary/view/effects unchanged;
- both reader/spender arrival orders, conditional cycles, RBF overlap,
  self-eviction, short-ID collision and wtx-cache tests pass;
- phase production lines are net-negative.

### Whole-architecture review

Re-audit all removal authority, graph roles, work bounds and effects as one
model. If sparse overlay requires a second mutable PoolMap or nested snapshot,
P2 fails and returns to design audit.

## 6. P3 — Stable effect shell cutover

### Work

- Keep the effect journal, global sequence/version clocks and DefectDomain in
  the stable kernel shell; entry-generation swaps cannot discard them.
- Replace dynamic effect reservations/population formulas with preallocated
  ordinary Remote ceiling + trusted headroom and a replaceable constant-size
  latest chain-authority register.
- Account bounded ingress requests before admission and make ordinary capacity
  a final Plan predicate. No state lock waits for capacity.
- Add generation-checked endpoint publication, publisher restart, bounded relay
  retry/reconcile, callback panic/hang isolation and validated endpoint-count
  permits. A chain/admin plan degrades oversized/full detail to
  `GenerationReset` before mutation.
- Delete generic EffectOutbox/reservation IDs, credit-across-lock paths and
  journal-triggered service fail-stop.

### Exit gates

- exact slot/byte accounting and largest-indivisible-batch startup checks pass;
- Remote saturation cannot consume trusted/critical progress;
- two consecutive critical authorities converge to the newest even when the
  older record is queued/active;
- pre-admission cancellation, relayer full/close, callback re-entry/panic/hang
  and publisher panic tests pass;
- phase production lines are net-negative.

### Whole-architecture review

Trace every state mutation to its prepared effect and every endpoint payload to
an owner/permit. Confirm entry-generation swap preserves committed ordinary
records and critical authority. Reject any new deferred payload channel.

## 7. P4 — Chain, persistence and DefectDomain convergence

### Work

- Move bounded detached replay into charged/persistable
  `RecoveryRetained(session,ordinal)` and direct trusted drain; delete
  `recovery_lock` and handler-local replay ownership.
- Implement exact reorg Gap/Proposed/Pending classification, proposal-wins
  optional-uncle filtering, immediate blank authority and latest-generation
  full refresh.
- Require every returned template to match current chain generation/parent;
  preserve reset/full mutual exclusion and same-generation full priority.
- Implement v2 explicit-save snapshot of causal-parent-first accepted and
  recovery-owned raw items, charged snapshot permit and serialized atomic
  writers.
- Implement touched repair, accepted cold rebuild, over-bound generation reset,
  one prebuilt spare, one DisposalPermit and one Remote DefectGate. Catch Plan,
  worker, endpoint and Apply unwind at the frozen failure boundary.
- Add metrics/logs for repair/reset/gate, Remote saturation/identity floor,
  effect regions, Wait backlog, chain recovery and template generations.

### Exit gates

- no `recovery_lock`, authoritative fail-stop or uncertain-state persistence;
- reorg/clear/save/cascading-reorg/restart/template matrix passes;
- hostile high-fanout chain event yields one observable reset while chain/RPC/
  Local/Proposal/template authority remains live;
- production build profile is unwind and fault injection cannot persist or
  expose partial Apply;
- phase production lines are net-negative.

### Whole-architecture review

Audit the complete failure domain and borrower memory equation, including old
generation destruction and in-flight worker/save/template Arcs. Verify chain
events cannot be rejected and a delayed hint cannot expose an old-parent
template.

## 8. P5 — Correctness, test isolation and review acceptance

### Work

- Move every inline production test body into dedicated test files/modules;
  test-only seams expose behavior, not mutable production bypasses.
- Rewrite old encoding tests against target invariants; delete tests whose only
  subject was a deleted mechanism while preserving their historical behavior
  mapping.
- Update `REVIEW_GUIDE.md` tables with invariant, attack, expected behavior,
  production boundary, unit/model/integration anchors and exact minimum/full
  commands.
- Regenerate inventory/manifest/checkpoint counts and production/test line
  totals.
- Run format, validators, `cargo nextest` for `ckb-tx-pool` including internal
  features, contextual dependent crates and clippy with zero new warnings.

### Exit gates

- all unit/model/fault tests pass in nextest isolation;
- every one of 152 findings and every policy row has live evidence;
- production source ≤14k or implementation stops for architecture re-audit;
- no tests/benchmarks hide production growth.

### Whole-architecture review

Repeat the complete signed matrix from `ARCHITECTURE_AUDIT.md` against actual
types and searches. Classify every deviation as implementation bug, deliberate
documented policy or architecture blocker.

## 9. P6 — Complete process integration acceptance

### Work

- Build once, list all registered specs, and execute the generated complete
  tx-pool impact universe through
  `make integration CKB_TEST_ARGS='-c 1 ...'`.
- Run families in deterministic small batches for diagnosis, then the complete
  universe in one acceptance command to expose isolation/order leaks.
- For each failure, preserve logs and classify root cause before editing:
  product defect, deliberate policy change with stale assertion, harness
  isolation defect, or unrelated environmental failure.
- Fix product defects only at the frozen authority/Plan/effect boundary; update
  stale tests only with behavior/guide evidence. Rerun the failing family and
  complete universe.

### Exit gates

- every registered impact spec passes under the documented command;
- no spec is silently filtered because it lives outside `specs/tx_pool`;
- manifest and guide match the actual final command/log verdict;
- no failure is waived as flaky without a reproduced harness cause and bounded
  correction.

### Whole-architecture review

Use integration observations to re-audit real RPC/relay/chain/miner semantics,
especially Local synchronous results, reorg/template priority and failure
containment. A failure revealing a design contradiction returns to the previous
checkpoint and architecture audit.

## 10. P7 — Performance and production acceptance

### Work

- Audit benchmark workload correctness, setup cost, invariant assertions,
  sampling variance and timeout/noise before using results.
- Use separate clean worktrees for `develop`, C1 and C8. Run repeated quick A/B
  first; attribute throughput, p50/p95/p99 latency, TxPool/kernel lock hold,
  allocations/RSS, resolve/verify utilization, commit/reorg/template latency and
  hostile fairness.
- Do not run the previously wasteful broad medium suite by default. Expand only
  a statistically inconclusive or regressed scenario, keeping workload and
  safety checks identical.
- Optimize measured causes only with frozen derived projections/hints. Any new
  owner/state/order/undo protocol is forbidden and returns to audit.
- Rerun correctness gates after every performance change and repeat quick A/B
  until no material regression remains.

### Exit gates

- throughput geometric mean does not decrease; repeated/statistically
  significant scenario regression blocks production;
- latency/RSS/CPU and hostile fairness meet the guide thresholds without weaker
  workloads or safety assertions;
- all P5/P6 tests still pass;
- final whole-architecture and attack audit has no unrecorded issue.

## 11. Per-phase review worksheet

Every phase commit contains answers/evidence for all rows:

| Dimension | Mandatory question |
|---|---|
| authority | Are `TxPool` and `PrePoolKernel` still the only executable payload owners? |
| state | Is every retained pre-accept payload in exactly one of seven variants? |
| identity/ABA | Are raw full hash, wtx cache key and one non-reused version used at their exact domains? |
| plan/apply | Can any ordinary failure occur after the linearization point or require rollback? |
| graph/wait | Are causal, conditional and availability relations distinct and event-safe? |
| effects | Is every required outcome prepared/charged before mutation and stable across reset? |
| resources | Do actual container capacities and every borrower fit the two envelopes? |
| locks | Does every nested path follow permit → TxPool → kernel with no I/O/work wait? |
| chain/template | Can any delayed effect/status/uncle expose a stale-parent or stranded-pending template? |
| failure | Can legal hostile input stop the service, persist uncertain state or create a reset loop? |
| policy | Is every observable difference in section 22.4 and the review guide? |
| performance | Is work edge/cohort bounded, source net-negative, and any optimization evidence-driven? |
| evidence | Do unit/model/integration anchors test behavior rather than deleted encoding? |

The worksheet is the anti-snowball gate. Finding a problem is not permission to
add a mechanism; it first asks which frozen equation was violated and whether
the existing design already supplies the correction.
