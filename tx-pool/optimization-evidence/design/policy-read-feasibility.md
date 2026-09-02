# Historical compile-time PolicyRead feasibility slice

This file records one historical feasibility experiment. It is not current
tx-pool architecture, control state or proof. Current status and the known
`ExclusiveMode` shared-barrier blocker are in
`tx-pool/control/txpool-v8/`; source and the manifest-bound findings win on
conflict with the conclusions below.

Base identity: `fbabfde74ac2acbba05c169002a6c1b34f251f5f`
(`4b66250d7e8624d9e65a0cd3b55b8b880cac5967`). This worktree is an
isolated feasibility experiment; it does not change the primary checkout.
The final authority-hiding hardening is based on the first feasible slice,
`5289a14c9a90e9a65e15d0a95ba4d4f7beb8b50e`
(`543d6635dcbc5fe6578984b823633c2d322a0691`).

## Question and boundary

Can the one canonical membership evaluator be generic over a sealed read
capability so that the compiler selects either:

- an `Exclusive` reader which performs the established direct point reads,
  returns borrowed Accepted guards, records no rows and takes no coherence
  cut; or
- an `Optimistic` reader which records the existing bounded exact witness and
  proves one sorted mixed cut before its result becomes usable?

The experiment must not copy the evaluator, add a second delta engine, change
policy, or migrate another runtime route. Projection compilation after policy
is out of scope: its raw reads construct the existing write prestate and are
already revalidated by Apply.

## Read census

The canonical call graph begins at
`evaluate_membership_policy_with_dependency_bound`, enters
`rbf::replacement_removals` and `eviction::complete_removals`, then uses the
shared traversal helpers and `VirtualProjection`.

| Mutable premise | Current canonical read sites | Existing evidence |
| --- | --- | --- |
| owner kind/version/vacancy | `observe_owner`, `observe_accepted_owner`; RBF fee/input and virtual aggregate reads | `ObservedOwnerRead` |
| spender | RBF conflict/input release and candidate output children | `ObservedSpenderRead` |
| dependency consumers plus staged visibility | late Accepted children | `ObservedDependencyConsumerRead` |
| parents/children | descendant closure, ancestor traversal, virtual removal/candidate application | `ObservedCausalRead` |
| ancestor/descendant aggregates | virtual projection and order-key construction | `ObservedAggregateRead` |
| accepted/eviction key membership | final aggregate/order delta construction | key plus observed presence |
| capacity/order frontier | bank fit probe, then `VirtualProjection::next_eviction` calls raw population `eviction_order()` only after capacity entry | explicit `capacity_frontier`; shared conversion refuses it |

The conservative batch classifier in `membership/independent.rs` also reads
these rows directly, but it is not the canonical policy conclusion. It remains
an optional batching gate and is outside this feasibility slice. Direct
owner-free capture is likewise already a separately sealed exact receipt.

The main abstraction leak is not a missing witness field: helpers receive both
`&TxPoolAuthority` and `&mut MembershipPolicyWitness`, so a future raw read can
bypass recording. The runtime `Disabled/Exact` branch also remains on every
read and the borrowed/captured Accepted carrier is a large runtime enum.

## Proposed bounded experiment

Introduce a private sealed `PolicyRead` trait with an associated
Accepted-entry carrier. Provide exactly two implementations:

1. `ExclusivePolicyRead`: direct reads, associated
   `ShardedAcceptedReadGuard<'authority>`, no witness storage.
2. `OptimisticPolicyRead`: delegates to the current exact recorder, associated
   owned/captured Accepted value, bounded dependency fanout, and returns the
   completed `MembershipPolicyWitness` only after coherence proof.

Make the canonical RBF/eviction/traversal functions generic over this reader.
There remains one source evaluator; monomorphized machine code is not a second
policy engine. Every owner/spender/dependency/causal/aggregate/order policy
read must be a trait method. Capacity population order must at least be routed
through the capability, although full 64-way capacity migration remains out
of scope.

## Success and stop conditions

Success requires:

- no canonical policy read in the censused families bypasses `PolicyRead`;
- `Exclusive` records zero witness rows and returns borrowed Accepted guards;
- `Optimistic` preserves all existing witness/interposition canaries;
- the original 14 focused tests plus `check`, `clippy`, formatting and diff
  checks pass.

Stop rather than force the abstraction if associated guard lifetimes require
tying returned values to `&mut reader`, if generic parameters spread into
`ProjectionDelta`/Apply/resource delta types, or if the traversal algorithms
would need duplicated Exclusive and Optimistic implementations. In that case
this document is the bounded result and the recommended next slice is a
larger, explicitly owned reader-boundary refactor.

## Feasibility result

The bounded implementation succeeded without crossing the stop boundary.
`PolicyRead` is private and sealed and uses an associated Accepted carrier; a
GAT was not required because each concrete capability owns the authority
borrow that anchors its carrier. `ExclusivePolicyRead::Accepted` is the original
`ShardedAcceptedReadGuard<'authority>`, while
`OptimisticPolicyRead::Accepted` is an owned `CapturedAccepted` newtype. The
canonical evaluator is generic once and selects the implementation at its
entry point.

RBF, descendant/ancestor traversal, dependency-consumer filtering and
`VirtualProjection` are generic over the capability. The old non-policy
administrative/projection callers use thin `ExclusivePolicyRead` wrappers over
the same traversal algorithms; no traversal or policy algorithm was copied.
Capacity's population order, immutable configuration and the existing
ResourceCapacityBank/resource aggregate are routed through narrow capability
methods. Full capacity migration is still deliberately out of scope.

The generic parameter ends at `MembershipEvaluation`; it does not enter
`ProjectionDelta`, Apply, resource deltas, scheduler/dependency deltas or the
runtime. The original focused 14 tests exercise both monomorphizations:
Exclusive still reports zero witness rows/captures and Optimistic retains every
stale/interposition canary.

The final bounded hardening removes that remaining abstraction leak. Each
sealed reader now owns its `&TxPoolAuthority`; the canonical evaluator, RBF,
eviction, graph traversals and `VirtualProjection` receive only `&mut R` plus
candidate-local immutable values. Configuration, resource-bank fit, exact
accepted-resource projection, accepted limits and capacity order are exposed
as narrow reader methods. None of those algorithms can name the raw entries,
membership or dependency authorities, so a future unrecorded mutable premise
read is no longer expressible without deliberately expanding the sealed
capability's trusted implementation.

The non-policy wrappers construct an Exclusive reader and call the same
associated traversal functions. Those functions have no `self` or authority
parameter; their placement in the existing implementation block does not grant
access to an authority instance. The extra generic parameter still terminates
at `MembershipEvaluation` and did not spread into ProjectionDelta, Apply,
resource delta types, scheduler/dependency deltas or runtime.

The hardening is a net-small change over the feasibility slice: resource logic
moved behind three narrow operations and the obsolete runtime
`records_witness` branch disappeared. It does not create a parallel context
object, raw-reference bundle or second evaluator. The remaining TCB is the two
sealed reader implementations plus the pre-existing exact witness recorder;
reviewers can census raw authority reads there rather than throughout the
policy call graph.

The same typed reader is suitable for the local-removal contract: closure walk
must record traversal-time child and owner facts and revalidate them in the
final mixed cut. Reusing `OptimisticPolicyRead` avoids a second causal-relation
receipt. Population-wide administrative input collection remains outside that
shared route.

## Validation evidence

- `cargo check -p ckb-tx-pool --tests`: passed.
- Focused canonical/witness suite: 14/14 passed, final Nextest run
  `e64f5baa-73bb-4468-a594-a56c7e447637`.
- `make check`: passed across all targets/features.
- `make clippy`: passed with the repository warnings-denied policy.
- `cargo fmt --all -- --check` and `git diff --check`: passed before freeze.
