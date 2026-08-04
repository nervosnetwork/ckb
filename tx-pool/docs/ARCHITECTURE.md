# Tx-Pool Architecture: Unified Authority Kernel

This document is the normative design of the current tx-pool. It describes
the surviving production model, not the migration history. Executable behavior
and attack regressions are indexed by [REVIEW_GUIDE.md](REVIEW_GUIDE.md);
machine-contract maintenance is documented by [VALIDATION.md](VALIDATION.md);
performance evidence and measurement rules live in
[PERFORMANCE.md](PERFORMANCE.md) and [BENCHMARK.md](BENCHMARK.md).

## 1. Decision

`TxPoolAuthority` is the only transaction-lifecycle authority. It lives with
the exact chain snapshot it validates against inside one `AuthorityStore`
protected by a synchronous `RwLock`. Its `entries` map owns every retained or
accepted transaction by raw transaction hash:

```text
Owner(h) = Nowhere
         | PreAccepted(phase, source, evidence, charge)
         | Accepted(status, proof, charge)
         | ReplacementHistory(observation, charge)
```

Indexes, membership relations, scheduling, dependency observations, resource
accounting, source versions, peer-ban state and committed effects are fields or
projections of that same authority. They change in the same total Apply as the
owner. None is a second decision authority.

Resolve, script verification and immutable planning are parallel work outside
the authority guard. `EntryVersion` plus a move-only checked-out work value
binds each compute result to the exact owner and chain view it can settle.
External I/O runs only after Apply and consumes effects committed by that
Apply.

```mermaid
flowchart TB
    subgraph Inputs["Ingress and chain inputs"]
        Remote["Remote"]
        Proposal["Proposal"]
        Recovery["Recovery"]
        Local["Local RPC"]
        Chain["Ordered chain transition"]
    end

    subgraph Store["AuthorityStore: one synchronous RwLock"]
        Snapshot["Arc<Snapshot> + ChainViewId"]
        Owners["TxPoolAuthority.entries<br/>PreAccepted | Accepted | ReplacementHistory"]
        Derived["Indexes | membership | scheduler | dependencies<br/>resources | source versions | peer bans | EffectLog"]
        Owners -->|"same total Apply"| Derived
        Snapshot -->|"paired evidence"| Owners
    end

    Compute["Parallel resolve and verify<br/>move-only versioned work"]
    Plan["Typed evidence -> closed Plan<br/>semantic state unchanged"]
    Effects["Sole effect publisher<br/>post-commit external I/O"]
    Reads["RPC | persistence | relay rebuild<br/>immutable coherent receipts"]
    Template["Versioned block-template lanes<br/>derived and rebuildable"]

    Remote --> Owners
    Proposal --> Owners
    Recovery --> Owners
    Local --> Compute
    Chain --> Store
    Owners -->|"checkout"| Compute
    Compute --> Plan
    Local --> Plan
    Plan -->|"single-use total Apply"| Owners
    Derived -->|"committed effect lease"| Effects
    Store --> Reads
    Store --> Template

    classDef authority fill:#dbeafe,stroke:#1d4ed8,stroke-width:2px;
    class Owners,Derived,Snapshot authority;
```

The word pipeline refers to parallel computation around this kernel. It does
not mean a chain of owning queues. The pipeline may reorder expensive work;
only Apply decides ownership and membership.

## 2. Compatibility boundary

The refactor preserves these externally observable contracts:

- Local RPC submission bypasses retained queues, performs validation directly
  and returns its committed result synchronously. It shares only the
  count-based transient compute semaphore, so a running Remote computation can
  briefly delay capacity acquisition without gaining ordering authority over
  Local work.
- Remote, Proposal and Recovery inputs use retained asynchronous validation.
- RPC continues to expose only `Pending` and `Proposed`. Internal `Gap` maps to
  public `Pending` for compatibility, but internal proposal/template selection
  always consumes the exact `AcceptedStatus` and never the RPC projection.
- Replacement history is private recovery state. It is absent from live-pool
  RPC status, block templates and persistence.
- The `get_raw_tx_pool.conflicted` field remains wire-compatible but now lists
  only successfully displaced Accepted victims retained as charged
  `ReplacementHistory`. A rejected conflict/RBF candidate is terminal
  recent-reject evidence and is never retained merely to populate that list.
  This is an intentional semantic narrowing of `develop`'s uncharged conflict LRU:
  per-hash callers use `get_transaction` for the rejection, while removing the
  candidate's second residency closes the freeloader and implicit-retry
  surface. Rolling back to `develop` restores the broader legacy list together
  with that cache behavior.
- Verification-cache identity is the inline 32-byte witness hash together with
  the exact `ScriptVerificationRules` generation.
- Persistence remains best effort. Accepted transactions and Recovery-source
  entries are captured from one read cut; all replayed transactions re-enter
  ordinary validation.
- Full/reset block-template replacement remains serialized. Proposal,
  transaction and uncle component work remains optimistic and concurrent.
- Consensus validation is not replaced or weakened. Tx-pool-only evidence
  reuse is explicitly scoped in section 5.

Any change to wire, RPC, persistence, cache or template behavior requires a
separate compatibility decision; an internal simplification is not permission
to change it.

## 3. Why structural change from `develop` is necessary

The justification is concrete failure closure, not preference for new types.
The table also records the cost paid by the UAK and whether risk is eliminated,
bounded or merely transferred.

| Family | Verified `develop` structure and failure | Why a local patch is not a complete proof | UAK mechanism and cost | Disposition |
|---|---|---|---|---|
| F1 ownership and ABA | `VerifyQueue`, `OrphanPool`, active verifier work, conflict storage and accepted `PoolMap` independently retain or infer a transaction's location. Clear, peer removal and re-admission cross those structures. | Locking one handoff still leaves every other producer, cancellation and administrative path to coordinate the same partition. A complete local fix would have to introduce a common owner/version protocol, which is the structural change. | One `entries` map; three owner variants; one non-reused entry version and move-only checked-out work. One authority write guard sequences short transitions. | Ownership ambiguity is eliminated. Lock contention is a measurable cost, not hidden. |
| F2 RBF and capacity atomicity | `process_rbf` removes victims and publishes rejection/conflict state before `_submit_entry` and `limit_size` have proved the replacement can remain accepted. | Moving one fee check earlier cannot make victim closure, descendant removal, accepted capacity, resource accounting, callbacks and failure recovery one transaction. Undo would add another fallible state machine. | Immutable membership/RBF compilation followed by one total Apply; optional victim history is installed by that Apply. Planning and closure work are bounded. | Partial replacement is eliminated. Bounded plan cost replaces rollback risk. |
| F3 observable effects | Accepted mutation, callbacks, relay-known state, recent rejection, cache work and block-template notification occur in separate calls/tasks and can be separated by failure or saturation. | Adding retries to every endpoint cannot prove which state was committed and can duplicate or omit publication. | `EffectLog` is part of the authority; a bounded effect delta commits with ownership. One move-only publisher consumes it after the guard opens. | State/effect gaps are eliminated in-process. Crash durability and exactly-once external delivery remain explicit residual risks. |
| F4 hostile resources | Accepted capacity, orphan count, verification cycles, conflict retention and auxiliary graph/index memory use different limits or omit metadata/active-work cost. | Independent ceilings do not prove the sum and allow transitions between structures to escape or double charge. | One `ResourceLedger` charges entries, bytes, edges, active work, compute envelopes, accepted cost, remote/per-peer state and replacement history; effect memory is separately bounded by the same construction inputs. | Uncharged residency is eliminated; conservative overcharging is an accepted bounded availability trade-off. |
| F5 dependency progress | Missing work, orphan work, accepted causal edges and conflict recovery use separate wake mechanisms. Definitive parent loss and source promotion can miss the mechanism that parked a child. | Adding a wake at one removal site leaves reorg, RBF, expiry, clear and peer revocation as independent publication tables. | One canonical dependency set, `DependencyFrontier`, observation cut, reverse keys and bounded level-triggered maintenance. | Lost-wake timing dependence is eliminated; bounded maintenance latency remains. |
| F6 conflict recovery | RBF victims are removed, conflict history is recorded separately and recovery is spawned later into another queue. Ordering decides whether a loser is restored, replaced again or stranded. | Restore/abort ordering patches move the race and fee/accounting proof between structures. | Only an actually Accepted victim can become charged inert `ReplacementHistory` in the same successful replacement Apply. Recovery always re-enters validation. | Speculative victim ownership and nested undo are eliminated. Optional history may be dropped as a complete set under its hard sub-budget. |
| F7 chain and template convergence | Reorg processing updates blank template/candidate uncles, then pool status/recovery, then full template. `Gap` and RPC `Pending` are conflated externally, and detached-uncle proposals can suppress re-proposal. | Fixing one `Gap` demotion or uncle filter leaves three independently published generations and startup/readiness loss. | One reliable capacity-one tip transition pairs snapshot and authority Apply; exact source versions drive versioned template receipts. Full/reset and optimistic component lanes retain their distinct concurrency. | Chain/owner generation splits are eliminated. A derived template can retain its last valid value and temporarily underfill after a rebuild failure. |
| F8 identity and evidence | Raw hash, witness hash and proposal short ID are passed through APIs with overlapping primitive types; detached recovery historically queried a witness-keyed cache with the raw hash. | Call-site review cannot prove every future caller selects the same identity and rule context. | Newtypes for owner/proposal/version/view identities and a concrete cache key containing witness hash plus script rules. Short IDs are collision-aware indexes only. | Identity substitution is made unrepresentable at typed boundaries. |

The concrete comparison anchor is `develop` at `91b97ab5f`. Reviewers can
reproduce the necessity trace without relying on this prose:

- `tx-pool/src/process.rs::submit_entry` calls `process_rbf` (which removes and
  publishes victims), then `_submit_entry`, then `limit_size`, and only later
  spawns victim recovery;
- `_update_tx_pool_for_reorg` moves `Gap -> Proposed` and
  `Pending -> Gap/Proposed`, but has no `Gap -> Pending` transition;
- `fetch_txs_verify_cache` keys the detached cache by witness hash while
  `readd_detached_tx` queries the returned map with raw transaction hash;
- `update_block_assembler_before_tx_pool_reorg` inserts every detached block
  into candidate uncles before pool reconciliation;
- `TxPool::package_proposals` excludes proposal IDs from all selected uncles,
  while `CandidateUncles::prepare_uncles` can retain the first detached block
  whose parent remains on the new main chain for the rest of the epoch;
- `PoolMap::pending_size` combines Pending and Gap, while `get_proposals`
  iterates only Pending, making the stranded state externally look healthy.

Each item crosses more than one owner/publication surface. Patching only the
last observed symptom leaves the other handoffs unproved; F1-F8 are the common
structural closure.

### 3.1 Necessity and cost ledger

Every retained mechanism below pays for a named root family. This table is the
review boundary against risk transfer and accidental complexity.

| Mechanism | Required for | Concrete cost and bound | Risk disposition and removal rule |
|---|---|---|---|
| `OwnedTx` in one `entries` map | F1, F2, F4, F5, F6 | Three resident variants under the configured count/byte/edge budgets. `Nowhere` is absence, not a fourth stored state. | Eliminates inferred ownership. A new owner variant is forbidden without a new business/attack case and continuous charge proof. |
| One synchronous `AuthorityStore` `RwLock` and total Apply | F1, F2, F7 | One short write section for owner/projection/charge/effect changes; shared coherent reads; no await, VM, I/O, large destruction or template build under the guard. | Eliminates partial commits while concentrating contention. Sharding is admissible only with a proof of cross-shard RBF/chain atomicity and measured benefit. |
| `EntryVersion`, `ApplySequence`, `PoolGeneration`, `ChainViewId` | F1, F5, F7, F8 | One version per owner, two process clocks, and revision plus tip identity. Checked `u128`/`u64` advancement; no duplicate compute counter remains. | Eliminates ABA, same-tip refresh ambiguity and hand-authored publication cuts. A token with no unique invalidation/publication role must be removed. |
| Membership, dependency, scheduler, resource and source projections | F2, F4, F5, F7 | Derived indexes changed by the same Apply; no duplicate payload owner. Ordinary transitions are local/bounded; named cold scans are listed in section 14. | Eliminates repeated population inference. Any projection that becomes a decision authority or lacks a rebuild/validation rule is rejected. |
| Count-only compute semaphore and worker topology | Parallel validation plus F4 | Bounded permits and configured resolver/verifier tasks. The semaphore contains no transaction identity; checked-out work remains move-only and cancellation-owned. | Bounds transient hostile work without serializing validation. Remove or resize only from profiling and resource evidence. |
| Bounded `EffectLog` and sole publisher | F3 and peer/refetch security | Three capacity regions, one claimed publisher task, exact progress lease, and startup-proved indivisible batch limits. Endpoint I/O is outside the lock. | Eliminates in-process mutation/publication gaps. Crash durability and universal exactly-once delivery remain external risk, not hidden owner state. |
| `ReplacementHistory` | F2, F6 | Inert optional owner under a separate hard count/byte/edge sub-budget; never scheduled, persisted or exposed as live RPC state. | Eliminates speculative victim restore/undo. Saturation drops the complete optional set while preserving the winner; no partial history is legal. |
| Capacity-one ordered chain lane plus five template lanes | F7 | One reliable reorg task; full/reset share one ordered replacement lane while proposal/transaction/uncle/notification work stays optimistic and derived. | Eliminates chain/authority split without serializing template construction. Any topology change must preserve the established lane concurrency. |
| 100,000-entry committed-hash compatibility cache | Compact-block lookup compatibility | Bounded LRU from proposal short ID to raw hash, committed with the paired snapshot. Full raw-hash and short-ID checks occur before a transaction is returned. | Collision can only underfill a derived compact-block response; it cannot select an owner or wrong payload. Delete when the compatibility lookup no longer needs it. |

The UAK is justified only while each retained mechanism maps to one of these
families or another named compatibility/attack requirement. An implementation
artifact with no such owner is design debt and must be removed.

## 4. Authority algebra and invariants

### 4.1 Owner variants

`OwnedTx` is a closed enum:

- `PreAccepted`: retained admission with source, original basis, current phase
  and exact charge;
- `Accepted`: consensus-facing pool member with status, proof, provenance and
  accepted accounting;
- `ReplacementHistory`: an inert, charged former Accepted victim retained only
  for bounded recovery.

`Nowhere` is absence from `entries`; it is not a stored tombstone. Every owner
stores a `TxRecord` whose raw identity equals its map key.

### 4.2 PreAccepted phases

`PreAcceptedPhase` has five semantic locations:

```text
Queued(Resolve)
Queued(Verify(ResolvedFacts))
Computing(ActiveWork)
Waiting(ObservedDependencies)
Ready(VerifiedFacts)
```

Resolve and Verify queue states share a typed `QueuedWork` enum. Computing
contains the only active work record, exact chain view, work permit, resource grant,
attribution, payload policy and dependency cut. Waiting can represent only a
non-empty missing-dependency observation. Replacement conflict history cannot
be encoded as executable waiting work.

### 4.3 Accepted status

`AcceptedStatus` is `Pending`, `Gap` or `Proposed`. Status is authoritative
internal state. Public RPC compatibility mapping is a derived read operation
and cannot feed proposal or commit selection.

### 4.4 Stable proof obligations

| ID | Invariant |
|---|---|
| T1 OwnerPartition | For every raw hash, `entries` contains zero or one `OwnedTx`; no worker, queue, effect, cache or template owns lifecycle state. |
| T2 CapabilityAndABA | A compute completion mutates only the exact entry version and Computing phase it checked out; chain-bound proof is retained only under its exact chain view. The checked-out work value is move-only, and stale work is mutation-free. Other asynchronous receipts carry their own typed generation or source cut. |
| T3 ContinuousResources | An owner is charged if and only if it exists; ownership and every charge coordinate change in the same Apply. |
| T4 DependencyExactness | Canonical input/cell-dep/header/dep-group facts and reverse observations describe the same owner generation. Definitive loss cannot leave a surviving accepted consumer. |
| T5 SchedulerExactness | Every executable PreAccepted owner has exactly the scheduler membership implied by its phase; no inert/history owner is executable. |
| T6 TotalApply | A prepared transition contains all owner, projection, resource, clock and effect deltas. Consuming Apply has no ordinary failure result. |
| T7 CommittedEffects | Every required external outcome is in the same committed Apply; the publisher never reconstructs it by rereading authority state. |
| T8 BoundedHostility | Peer-controlled count, bytes, edges, fanout, closure, retries, active work and effect memory have explicit checked bounds. |
| T9 ChainSnapshotPairing | `Arc<Snapshot>` and `ChainViewId` move as one store fact; a T -> T' -> T sequence remains distinguishable by revision. |
| T10 LevelTriggeredProgress | Notifications are hints only. A consumer subscribes before checking its authoritative level and sleeps only when a named independent action can change that level. |
| T11 IdentityAndEvidence | Every proof is bound to the exact raw/witness identity, script rules, chain view and policy context it proves. |
| T12 CoherentPublicProjection | RPC, compact-block, persistence, relay rebuild and fee/template reads are captured from one authority read cut and finished outside the guard. |
| T13 TemplateConvergence | Template lanes publish only receipts whose chain/source cut remains current; full/reset priority and optimistic partial concurrency are preserved. |

These invariants are checked by construction first and by bounded validation as
defense in depth. A runtime check is not a substitute for a type that can
remove the invalid state.

## 5. Identity and evidence

| Type | Role |
|---|---|
| `RawTxHash(Byte32)` | primary ownership and accepted membership |
| `WitnessTxHash(Byte32)` | transaction witness identity inside authority evidence |
| `TxVerificationCacheKey { [u8; 32], ScriptVerificationRules }` | copy-cheap, context-complete script cache identity |
| `ProposalId(ProposalShortId)` | collision-aware consensus proposal index only |
| `EntryVersion(u128)` | non-reused owner and active-compute identity; source-only transitions may deliberately preserve it |
| `CheckedOutWork` / `SettlementToken` | sole move-only active computation capability; it carries `EntryVersion` rather than a redundant second counter |
| `ApplySequence(u128)` | total committed authority/source/effect order |
| `PoolGeneration(u64)` | clear/generation boundary |
| `ChainViewId { ChainRevision, ChainTipHash }` | exact chain event and same-tip evidence scope |

Positive chain-cell evidence may be reused only when the current and resolved
tip hashes match and the individual `CellMeta.transaction_info` proves the cell
came from the chain. Pool-produced cells are always checked against the mutable
membership overlay. This is a tx-pool final-admission optimization only; it is
not available to block or consensus verification, removal-history reasoning or
another transaction's dependency proof.

`CellLocationReceipt` owns the exact `ChainViewId` captured with those roles;
`VerificationContextReceipt` consumes that receipt rather than accepting a
separately supplied view. Resolver, final-validation, direct and internal
construction therefore cannot express a location/view mismatch.

The `pre_resolve_tip`, cell metadata and script-rule generation must originate
from the same captured `Arc<Snapshot>`. No caller may infer provenance from an
ingress source or a nearby hash.

## 6. Lifecycle and sources

```mermaid
stateDiagram-v2
    state "Nowhere" as N
    state "Queued Resolve" as QR
    state "Queued Verify" as QV
    state "Computing" as C
    state "Waiting Missing" as W
    state "Ready" as R
    state "Accepted Pending/Gap/Proposed" as A
    state "ReplacementHistory" as H

    [*] --> N
    N --> QR: Remote / Proposal / Recovery admission
    N --> C: Local direct validation capture
    QR --> C: checkout Resolve or ResolveThenVerify
    QV --> C: checkout Verify
    C --> QV: resolved
    C --> W: missing dependency
    W --> QR: dependency level advances
    C --> R: verified but not admitted now
    R --> A: final Plan + Apply
    C --> A: proven independent/direct final Apply
    A --> H: successful RBF retains victim
    H --> QR: typed recovery trigger
    A --> QR: detached-chain Recovery
    QR --> N: reject / expiry / peer revoke / clear
    QV --> N: reject / expiry / peer revoke / clear
    C --> N: reject / cancellation / chain removal
    W --> N: definitive loss / expiry / peer revoke / clear
    R --> N: reject / admin / chain removal
    H --> N: saturation / terminal cleanup
```

Sources are policy, not locations:

- Remote carries immutable ingress peer/residency and the declared-cycle
  policy for its exact witness payload. It is charged globally and per peer.
- Proposal is trusted. Same-witness promotion may preserve immutable ingress
  attribution while replacing peer-supplied verification policy.
- Recovery is trusted detached-chain/persistence input and must re-run normal
  resolve and verification.
- Local owns no retained phase. It performs computation directly and uses the
  same final membership compiler and effect rules.
- TestAccept evaluates validation policy but performs no authoritative Apply.

A peer ban removes only not-yet-Accepted owners attributed to that peer. Its
committed cohort effect resets the relay projection. Because relay marks input
known before asynchronous controller delivery, every already-queued message
that reaches the live peer fence later commits an exact
`RemoteIngressReleased` effect of its own. Another peer can therefore supply
the same raw transaction in either ordering; Accepted owners remain accepted.

## 7. Validate, Plan, Apply and effects

### 7.1 Validation

Resolution and script verification consume immutable snapshot/overlay inputs
and produce concrete receipts. Workers carry no owner map and never decide
membership. Resource envelopes are reserved before attacker-shaped resolved
data can become retained state.

### 7.2 Plan

Plan runs under a coherent authority cut and changes no authoritative semantic
fact. It may reserve collection capacity so Apply cannot fail allocation, then
builds a private closed delta containing:

- exact before/after owners;
- index and source-version replacements;
- accepted membership/causal projection changes;
- scheduler and dependency-frontier changes;
- resource charge changes;
- clock values;
- the exact bounded effect mutation;
- retirement carriers and, only for specialized checkout plans, the exact
  move-only worker or publisher capability.

Ordinary outcomes such as rejection, backpressure, duplicate, stale evidence
and cancellation are decided before Apply and represented by closed enums. A
prepared plan borrows the authority mutably and is `must_use`; it cannot be
applied twice or against another authority.

### 7.3 Apply

`PreparedApply::apply(self) -> CommittedDelta` is total. It swaps all fields in
one short authority critical section, advances the clocks and returns change
evidence plus outside-guard retirement. Large retired payloads and generations
are carried out of the guard before destruction. Apply performs no external I/O
and has no rollback path.

Specialized `PreparedCheckout` and `PreparedEffectCheckout` types contain their
move-only compute/effect capability beside the generic plan by construction.
Generic plans and `CommittedDelta` cannot carry either capability, so a
successful checkout cannot discover a missing handoff afterward and an
ordinary transition cannot manufacture one.

```mermaid
sequenceDiagram
    participant D as Dispatcher or worker
    participant S as AuthorityStore
    participant C as Parallel computation
    participant P as Effect publisher
    participant E as External endpoint

    D->>S: capture typed work + exact version/view
    S-->>D: move-only checked-out work and immutable evidence
    D->>C: resolve or verify without authority guard
    C-->>D: typed result
    D->>S: Plan under coherent cut
    Note over S: validate stale/budget/policy<br/>reserve capacity; build complete delta
    D->>S: consume total Apply
    S-->>D: typed capability + CommittedDelta retirement
    Note over D,S: authority guard opens before destruction or await
    P->>S: checkout committed effect progress
    P->>E: perform external I/O
    P->>S: settle exact effect progress
```

## 8. Membership, RBF and dependencies

### 8.1 Accepted membership

`MembershipProjection` is derived from Accepted owners and stores the causal
input/cell-dependency graph, spender relation, ancestor/descendant aggregates,
status counts and deterministic ordering required by admission and templates.
It is not a second payload owner. Membership preparation validates the complete
virtual final set before the authority owner map changes.

### 8.2 Independent and coupled work

Transactions whose inputs/dependencies are chain-backed and whose membership
plans commute may be validated concurrently and accepted through one bounded
`IndependentDelta`. Coupled transactions use the same authority and proof
rules but compile an exact cohort. This extracts CKB cell-model parallelism
without adding a sharded owner map or a second DAG authority.

### 8.3 RBF

RBF planning computes the complete conflict/victim/descendant closure, new
unconfirmed-input rule, ancestor/descendant overlap, dependency-on-victim rule,
candidate bound, absolute replacement fee and size-based fee-rate gate against
one coherent virtual membership. A rejected membership decision leaves every
Accepted owner unchanged; applying that decision terminalizes the candidate
with its rejection effect and never creates candidate `ReplacementHistory`.

A successful plan applies candidate insertion, all victim removals, descendant
updates, charge changes, dependency levels, source versions and effects once.
Only actually Accepted victims can become `ReplacementHistory`. If the history
partition cannot retain the complete optional set, the set is terminalized and
the winner is retried without history; partial history and winner failure are
not legal saturation outcomes.

### 8.4 Dependency progress

Every owner carries one canonical `KnownDependencies` basis. Missing work owns
a non-empty `ObservedDependencies` with an exact `DependencyCut`. Reverse keys
are derived in `DependencyFrontier`. Availability and definitive loss advance
authority levels in the same Apply that changes the producer or spender.

Positive cell evidence has two layers: an attached block may make an output
chain-live, but that output is not finally available while the post-Apply
Accepted membership still has a spender. Chain validation therefore carries
explicitly named chain-layer facts, and chain Plan filters them through the
already-compiled final membership projection before publishing a dependency
level. This same-cut composition prevents parent commitment from falsely
waking an RBF victim while its winner still owns the spend; it requires no
cache, second projection, repair scan or extra lock.

Workers subscribe to the mutation hint before checking the authoritative
level. The hint may be coalesced or lost because the level is the truth.
Maintenance operates in bounded slices and re-arms only when a relevant source
cut advances. A timeout may diagnose failure but is never the progress proof.

The production resolver must not expose a PreAccepted output as chain-backed
dependency evidence for an unrelated transaction. Accepted pool-produced
evidence is represented through the membership overlay and causal compiler.

## 9. Chain and administrative transitions

The chain layer owns one tip-installation boundary: install the new snapshot,
then send the exact fork delta on a reliable capacity-one ordered channel. RPC
readiness cannot suppress this transition. The ordered reorg driver packages
detached/attached inputs outside the authority guard, then one chain Plan/Apply
pairs the new `Arc<Snapshot>` and `ChainViewId` with all owner, membership,
status, recovery, dependency, resource and effect changes.

Normal best-block and truncate paths use this same boundary. IBD and
candidate-uncle notifications remain readiness-gated derived signals and must
not be placed on the authoritative ordered channel.

Clear, peer revocation, local removal, remote expiry and accepted expiry use a
cause-complete owner-removal compiler. It updates every projection and required
effect once. `ClearPipeline` preserves Accepted owners; `ClearPool` creates a
fresh empty generation. Reorg and clear do not use a recovery lock, nested undo
or detached-block payload owner.

## 10. Committed effects and external failure

`EffectLog` is a bounded field of `TxPoolAuthority`. It owns immutable committed
outcomes, not transaction lifecycle. Capacity is partitioned into Remote,
Trusted and Critical classes, each with an indivisible batch bound validated at
construction. Chain detail that every consumer can rebuild may collapse to one
constant-size `GenerationReset`; non-rebuildable security effects may not.

The effect enum includes accepted/rejected outcomes, chain-committed remote
settlement, peer-cohort revocation, remote expiry/release, parent requests and
generation reset. A pending-recent-reject index is a charged lookup into the
same resident batches.

One synchronously claimed publisher task is the sole consumer. It checks out a
move-only lease, executes endpoints after the store guard opens and settles the
exact progress token. Endpoint failure cannot roll back or reinterpret the
committed owner transition. Relay publication uses a bounded nonblocking
mailbox; overflow converges through `GenerationReset` and bounded level rebuild.

Effect-index inconsistency is an authority projection fault. JSON encoding,
storage, callback, network and other endpoint failures are derived or external
outcomes and cannot construct authority-generation invalidity.

Effect delivery is not crash durable or universally exactly-once. This is an
explicit operational boundary, not hidden transaction state.

## 11. Reads, RPC, persistence and cache

All public/query projections borrow one `AuthorityReadView` from one store read
cut. Allocation-heavy sorting, parent expansion and serialization occur after
the guard opens. Cursors carry exact source cuts and restart if relevant state
changes.

- RPC may show PreAccepted work as Pending and maps Accepted Gap to Pending for
  compatibility; internal detail and template code consume exact phases/status.
- ReplacementHistory returns no live RPC status and falls through to the
  existing recent-reject/RBF-compatible surface.
- The legacy `get_raw_tx_pool.conflicted` projection contains only charged
  ReplacementHistory victims. It is not a list of every recent RBF rejection;
  those terminal candidates are available only through the per-hash reject
  surface and cannot consume hidden tx-pool residency.
- Compact-block lookup is collision-aware and may read every live owner that
  intentionally participates in that compatibility surface.
- Persistence captures Accepted owners and Recovery-source PreAccepted owners,
  orders them parent-first outside the guard and writes through one serialized
  atomic-file writer. It excludes Remote, Proposal and ReplacementHistory.
- Cache reads and writes use `TxVerificationCacheKey`; cache update is derived
  work and never gates committed ownership.

## 12. Concurrency, tasks and progress

### 12.1 Lock domains

The transaction authority has one synchronous `RwLock`. Production code may
not hold it across `.await`, blocking I/O, VM execution or external endpoint
work. `AuthorityRuntime` exposes concrete operations rather than a generic
mutation closure.

Every authority mutation remains directly visible in an `AuthorityRuntime`
method. After the write guard and retirement carriers open, that method emits
one top-level lossy mutation hint; no fallible or escaping control flow lies
between the committed mutation and the hint. A source validator enforces this
shape and rejects hidden Apply helpers, conditional post-commit publication or
an unpaired hint. This preserves lock-external coalescing without adding signal
state to the authority or maintaining a second transition-to-wake table.

Other locks protect derived outputs only: verification cache, relay mailbox,
candidate uncles, current block template and template convergence. They cannot
decide transaction ownership. Template publication acquires the current-
template guard, then performs a bounded synchronous authority source check;
production never waits on the template guard while holding the authority guard.

The count-only compute semaphore reserves transient capacity but contains no
transaction identity and has no independent lifecycle state.

### 12.2 Task ownership

`AuthorityTaskTopology` constructs every capability before spawning the first
task and owns:

- resolve/verify workers;
- Ready and bounded maintenance drivers;
- the sole effect publisher;
- the derived verification-cache updater;
- optional block-template lanes.

The service generation separately owns the ordered reorg driver. Cancellation
closes producers first, joins authority workers, closes and drains effects,
then joins derived tasks. Every task exit is classified by what it owns;
template/cache degradation retains authoritative state, while loss of the sole
authority capability forbids persistence. Section 15 records the completed
backward reachability proof that valid or hostile input cannot construct that
boundary.

### 12.3 Wait-for proof

Every wait must name an independently running releaser:

| Wait | Held authority capability | Releaser |
|---|---|---|
| compute semaphore | none | completion/cancellation of another checked-out computation |
| mutation `Notify` | none | every runtime Apply publishes once after its guard opens; waiter rechecks level first |
| effect capacity | the exact failed settlement capability, no store guard | sole effect publisher settlement or cancellation |
| verification cache channel | no owner or store guard | cache updater; cache failure is derived degradation |
| ordered chain channel | chain request at producer boundary, no store guard | ordered reorg driver |
| template source change | no authority guard | authority Apply or candidate-uncle source mutation |
| shutdown joins | topology owner only | cancellation-aware owned task or bounded operational timeout |

The complete deadlock/livelock/lost-wake/starvation audit and constructive
saturation tests are release gates, not assumptions inferred from timeouts.

## 13. Block-template convergence

The block assembler is a rebuildable derived projection. It owns no tx-pool
membership. `AuthorityTemplateReadReceipt` captures accepted payloads,
relations, exact proposal/transaction/chain source versions and the paired
snapshot under one read cut; construction happens after the guard opens.

Five tasks preserve the established performance model:

1. one ordered replacement lane serializes reset and full rebuild;
2. one optimistic proposal lane;
3. one optimistic transaction lane;
4. one optimistic uncle lane;
5. one coalesced external notification lane.

Each build publishes only if its chain/source receipt and expected template
revision/reset epoch still match. Full/reset has priority over partial work,
but partial and uncle construction is not serialized behind it. Optional
proposals and uncles share one byte budget; only proposal IDs actually
published in the candidate uncles are excluded. A rebuildable failure retains
the last valid template and waits for a source-level change rather than
spinning or mutating tx-pool state.
Candidate-uncle insertion is likewise a bounded derived observation: source
counter exhaustion rejects that cache mutation and records degradation, but it
cannot veto an already-committed chain Apply or forbid authority persistence.

## 14. Resource and complexity contract

`ResourceLedger` continuously charges each owner by raw hash. Coordinates are
entries, resident bytes, dependency edges, active work, reserved compute bytes
and edges, accepted count/size/cycles, global Remote usage, per-peer usage and
ReplacementHistory usage. Effect batches/bytes have a separate bounded ledger
inside the same authority.

Checked construction proves limit hierarchy and a complete per-lease compute
envelope. Transitions use checked arithmetic. Peer-controlled parent count,
dependency expansion, RBF candidates, causal closure, eviction cohort,
maintenance slice, relay mailbox and effect batch all have explicit bounds.

Population-sized work is forbidden in ordinary lock-held ingress/settlement.
The completed static complexity inventory is:

| Path class | Maximum work under the authority guard | Why it is admitted |
|---|---|---|
| Remote/trusted ingress, checkout and compute settlement | Transition-local index/projection deltas. Queue checkout visits at most the charged owner rows; Ready selects at most `MAX_READY_BATCH = 8`. | Hot path; no full owner scan or attacker-sized destruction. |
| RBF, eviction and accepted causal removal | Complete indexed conflict/descendant cohort capped by `MAX_POOL_MUTATION_CANDIDATES = 100`, with configured ancestor/descendant bounds. | Atomic membership requires the complete closure; over-bound input is rejected or the chain generation is rebuilt. |
| Dependency/expiry maintenance | One dependency edge/marker step, one accepted causal root closure, or at most `ADMIN_MAINTENANCE_SLICE = 32` due Remote owners per Apply. | Level-triggered bounded progress; repeated work yields between Apply cuts. |
| Ordered reorg | Work proportional to the actual fork plus bounded accepted closures. Over-bound recovery replaces the ephemeral generation instead of retaining partial state. | Chain generation is trusted consensus work and must reconcile as one ordered cut. Block traversal and payload compaction occur before the write guard. |
| `ClearPipeline` / `ClearPool` | All live owners in an explicit administrative command. | Deliberate whole-generation operation, never ordinary ingress. Retired payload destruction happens after the guard opens. |
| RPC, persistence, relay rebuild and template capture | Bounded read capture/page under a shared guard; sorting, serialization, parent expansion and template packing occur after it opens. | Coherent projection requires one read cut but not exclusive ownership. |
| Template graph algorithms | Outside the authority lock; selected dependency occurrences and descendant-cache memberships are each capped at 200,000 and conditional-cycle shedding at 64 rounds. | Derived consensus packaging with deterministic underfill fallback. |
| Candidate uncles and committed-hash cache | Hard limits of 128 and 100,000 respectively, outside lifecycle authority decisions. | Bounded compatibility/template projections; exhaustion degrades or evicts derived data only. |

No other population scan is admitted. Adding one requires an explicit row,
attack bound and complexity regression before implementation.

## 15. Rust-native failure model

Valid transactions, hostile peers, stale work, duplicates, capacity pressure,
allocation pressure, cancellation and external failure are typed ordinary
outcomes. They must not panic, deadlock, livelock, leak charge or invalidate the
service generation.

Production authority code does not use `panic!`, `assert!`, `unwrap()`,
`expect()` or `catch_unwind` as validation or control flow. A structural
`AuthorityFault` denotes contradiction between the one owner and its derived
projections, not policy rejection. Rust is not treated as an Erlang-style
restart runtime: panic-and-catch and broad fail-stop are not repair mechanisms.

The service error algebra encodes this boundary rather than maintaining a
variant allowlist: operational failures are direct `AuthorityServiceError`
variants, while only `AuthorityServiceError::Integrity(AuthorityIntegrityFault)`
can construct `AuthorityGenerationInvalidity`. Exhaustive matching therefore
forces every new service outcome to choose its failure domain at compile time.
The ordered chain consumer is narrower still: `AuthorityChainUpdateError`
contains only cancellation and integrity. Allocation retains and retries the
exact request; a future operational service variant cannot silently terminate
the sole reorg task.
Rebuildable candidate-uncle collection and response/config conversion remain
outside the integrity domain; a dedicated ingress-rejection commit proof keeps
unrelated successful dispositions unrepresentable at the Remote-pressure call
site.

The backward constructor audit is closed as follows:

| Integrity class | Only legal constructor premise | Why valid/hostile input cannot reach it |
|---|---|---|
| Counter exhaustion | Checked `EntryVersion`, arrival, Apply/source, pool-generation or chain-revision advancement fails. | Counters start fresh per process/generation and every input-driven step is bounded. Reaching `u128` or `u64` exhaustion requires an unsupported number of committed transitions, not a transaction shape. |
| Invalid chain evidence | Canonical fork facts contain duplicate transaction/header identity, or proposal positions disagree with the exact installed snapshot. | The sole chain producer supplies a consensus-validated fork and its paired snapshot through one reliable ordered boundary. Peer transactions cannot construct this command. |
| Resource projection | An existing owner/charge row disagrees, checked subtraction underflows, or a sealed membership compiler returns a resource outcome it had already proved impossible. | Ordinary allocation and all configured total/Remote/peer/accepted/compute/history limits are typed backpressure or rejection before Apply. |
| Membership/index/scheduler/dependency projection | Same-cut owner-derived structures disagree, or a supposedly exact move-only capability names no matching owner/phase. | Lock-external stale evidence is an ordinary stale result. Under one guard, these structures are changed only by the same total Apply and validated by model/projection regressions. |
| Effect projection | A prevalidated indivisible batch no longer fits its configured region, effect index/progress disagrees, or a rebuildable chain delta leaks raw capacity pressure. | Full/allocation/closed endpoint outcomes remain operational; only contradiction with the startup-proved effect algebra reaches this class. |
| Effect lifecycle closed during ordered reorg | A state producer observes the effect log closed before producer cancellation/join. | Shutdown owns producers-before-effect-close ordering. No input closes effects; occurrence proves a topology/programmer defect and cannot be ignored without losing the paired chain transition. |

This is not a panic-free or restart-based architecture. Structural faults
remain typed defense in depth for programmer defects; they are not validation,
policy control flow or a recovery mechanism. Any new constructor must repeat
this backward proof or be redesigned as a local typed outcome.

## 16. Performance model

The architecture preserves parallelism where CKB permits it:

- independent resolve and script verification run concurrently outside the
  authority lock;
- a worker may retain its transient permit across the exact computation it
  owns, but not across unrelated waiting;
- independent membership transitions may compile into bounded commuting
  batches;
- Local direct validation bypasses retained scheduling;
- authority read queries are shared;
- block-template component construction stays optimistic and parallel;
- external effects, cache writes and persistence I/O stay outside the guard.

Resolve and Verify queues use committed per-owner round-robin fairness. Ready
admission deliberately does not: it is strict `Recovery > Proposal > Remote`,
then fee rate, absolute fee, earlier arrival and deterministic identity/version
ties, in bounded batches of eight. No aging state exists. Remote expiry bounds
hostile retention; trusted work has no per-entry latency guarantee. This is a
documented policy trade-off, not a claim that every Ready owner is fair.

The cost of safety is short authoritative checkout/settlement/Apply work,
version/evidence objects, exact derived projections and a bounded effect log.
These costs must be measured, but no optimization may create another owner,
weaken final validation, move expensive work under the guard or serialize the
template lanes.

Static review precedes profiling. Profiling attributes cost and selects
optimization candidates; fixed-binary A/B decides measured value. Neither is a
correctness-discovery substitute.

## 17. Minimality and rejected alternatives

### Harden the `develop` queues

Rejected. Closing all owner handoffs, RBF rollback, resource transfer and
publication gaps would require a shared authority/version/effect protocol
across the queues. Retaining the queues after adding that protocol would keep
duplicate state without preserving useful concurrency.

### Separate pre-pool and accepted authorities

Rejected by the current implementation review. The cross-owner handoff and
coherent read problem were accidental complexity. Accepted and preaccepted
payloads now occupy variants in one map and one Apply; read concurrency comes
from the store `RwLock`, not a second owner.

### Universal async actor

Rejected. Serializing validation, read capture, template work and Local direct
submission through one mailbox would weaken independent-transaction and
read/template concurrency. The kernel serializes only ownership transitions.

### Nested undo or rollback

Rejected. All fallible policy, capacity, closure and effect checks finish
before total Apply. Undo would add persistent intermediate states and another
failure path without a business requirement.

### Separate DAG or sharded lifecycle owners

Rejected as a default. CKB dependency information is valuable for bounded
frontier scheduling and commuting batches, but a second DAG/shard authority
would duplicate membership, RBF and resource facts. The current dependency and
membership projections extract graph parallelism under one owner. A future
shard design must prove conflict/RBF/chain atomicity and lower measured cost
before adding state.

### No pipeline

Rejected. Removing retained parallel computation would simplify scheduling but
discard the main scalable property: chain-backed independent transactions can
resolve and verify concurrently. The correct simplification is one owning
kernel with typed borrowed work, not serialized computation.

This is the smallest constructively safe model selected by the completed
pre-benchmark minimality audit: every state, lock, task, clock, receipt and
effect class retains a business, compatibility, concurrency or attack owner.

"Smallest" here means semantic state and proof surface, not minimum source
lines. The explicit transition algebra is substantially larger than
`develop`, but it replaces implicit cross-queue protocols rather than adding a
second feature model. In particular, mechanically splitting the private
`plan.rs` algebra would currently require wider cross-module visibility without
removing a state or transition; that is navigation cleanup, not architectural
simplification. It should be attempted only as a zero-behavior mechanical
change after the production gates, with no new public constructors or traits.

Post-Apply values are part of that audit. They may carry a linear capability,
an outside-lock retirement carrier, a committed external effect or an
operational observation that a production consumer actually uses. They must
not duplicate owner hashes, causes, views, sequences or transition variants
solely to let tests reconstruct the mutation. Tests inspect a sealed Plan
before Apply and the authority/effect state after Apply; otherwise the test
receipt becomes a second hand-maintained projection of the transition graph.

### Historical finding convergence

Historical reports are evidence inputs, not parallel design documents. Their
current-code dispositions are retained here by root family so a future review
cannot silently reopen or forget them.

| Finding family | Current disposition | Surviving proof owner |
|---|---|---|
| Reorg-recovered `Gap` work and detached-uncle proposal suppression | `confirmed-closed` | Reliable paired chain Apply demotes status from the new proposal window; only actually published uncle proposal IDs suppress packaging; normal-mining reorg integration evidence is registered. |
| Ghost/double ownership, budget-exempt waiting/RaceLost, non-atomic victim removal | `superseded-by-proven-model` | One charged `OwnedTx` location and total membership/RBF Apply make the old cross-queue states unrepresentable. |
| Definitive parent death, dep-to-in-flight producer ambiguity, chain/overlay availability drift and lost dependency wake | `confirmed-closed` | Canonical dependency evidence, exact reverse frontier, same-Apply final membership projection, loss levels and bounded level-triggered maintenance. |
| Failed-RBF retry loop, missing rejection publication, public conflict-history status and partial history saturation | `confirmed-closed` | Deterministic terminal effect, private charged `ReplacementHistory`, complete-set saturation and full-validation recovery. |
| Manual cause/effect drift, including wrong `LocalRemoval` relay release and peer-ban refetch blockage | `confirmed-closed` | Cause-complete owner removal and exhaustive committed effect compilation; ban removes only attributed not-yet-Accepted owners and releases relay-known state. |
| Legal input/resource/external/template/cache outcomes promoted to fail-stop | `confirmed-closed` | Closed ordinary outcome enums, derived degradation and the backward fault table in section 15. The ordered chain task now exposes only cancellation or structural integrity. |
| Raw/witness/short-ID and script-rule identity substitution | `confirmed-closed` | Typed raw/witness identities, `TxVerificationCacheKey`, paired chain evidence and full-hash validation of short-ID compatibility reads. |
| `C-freeloader-rbf` under-fee restore-before-recover claim | `suppressed-with-current-counterevidence` | The alleged held under-fee candidate cannot pass the prerequisite size-based higher-fee gate; UAK no longer has the reported RaceLost restore ordering. |
| Strict/permissive marker allegedly required for same-tip cell reuse | `suppressed-with-current-counterevidence` | Chain-dead inputs cannot produce resolved evidence; per-input chain provenance plus exact snapshot/view is sufficient, and pool-produced cells always use the overlay. |
| Constructorless `Waiting(Conflict)` | `superseded-by-proven-model` | The variant was removed; only successful displacement of an Accepted victim constructs inert `ReplacementHistory`. |
| Phantom compute/proposal tokens and constructorless publisher collision | `confirmed-closed` | `EntryVersion` plus move-only work is the sole compute identity; the publisher is synchronously claimed before spawn. |
| Zero-match test anchors, partial integration selection, stale machine contracts and missing cross-crate CI triggers | `confirmed-closed` for static process contracts | One-way evidence discovery fails dangling symbols/test arms and derives CI roots from registered evidence. P9.8 execution remains a release gate. |
| Performance/noise claims from pre-UAK checkpoints | `superseded-as-release-evidence` | Historical profiles may select mechanisms, but only P10 fixed-current-binary A/B can accept the current architecture. |

## 18. Implementation map

| Concern | Primary implementation owner |
|---|---|
| owner algebra and typed evidence | `authority/state.rs`, `authority/chain.rs` |
| total Plan/Apply and clocks | `authority/plan.rs`, `authority/plan/` |
| accepted membership and RBF | `authority/plan/membership.rs`, `authority/plan/membership/` |
| resources | `authority/resources.rs` |
| scheduling | `authority/scheduler.rs` |
| dependency observations and wake | `authority/dependency.rs` |
| source-version compiler | `authority/source.rs` |
| committed effects | `authority/effect.rs`, `authority/publisher.rs` |
| runtime lock and capabilities | `authority/runtime.rs` |
| workers and task ownership | `authority/worker.rs`, `authority/topology.rs` |
| service compatibility boundary | `authority/service.rs` |
| chain packaging and ordered boundary | `authority/chain_boundary.rs`, `chain/src/verify.rs` |
| coherent reads and persistence receipts | `authority/read.rs`, `authority/query.rs` |
| template receipts and publication lanes | `authority/template.rs`, `authority/template_driver.rs` |
| bounded relay projection | `authority/relay.rs` |

Production tests belong under dedicated `tests/` modules. Any irreducible
`cfg(test)` field or observation hook must be a named seam in
`test-layout-manifest.json`; an entire production directory may never be
excluded from static safety review.

## 19. Change rules

Before adding a state, lock, task, queue, cache, version, effect or fallback:

1. name the business/compatibility/attack case;
2. identify the one authoritative owner and legal transitions;
3. prove why an existing type or derived projection cannot represent it;
4. define identity, validity, rebuild and resource bounds;
5. derive the wait-for and shutdown edges;
6. account for lock work, allocations, clones and concurrency;
7. add exact unit, integration, hostile and complexity evidence;
8. update the architecture contract, behavior registry and review guide in the
   same change.

Manual producer-to-consumer maps are suspect. Prefer one exhaustive compiler
from before/after owner state to source versions, effects and wake levels. A
variant with consumers but no producer, or a publication point with no
consumer/rebuild rule, fails review.

## 20. Residual risks and release conditions

The following are explicit boundaries, not claims of completion:

| ID | Residual risk or release condition |
|---|---|
| R2 | External effects are not crash durable or universally exactly-once. |
| R3 | Persistence is best effort and every replayed transaction re-enters validation. |
| R4 | Process OOM abort, FFI failure and memory corruption are outside the tx-pool model. |
| R5 | Derived template failure can retain the last valid template and underfill until a source change. |
| R6 | Optional replacement history can be discarded as a complete set under its bounded sub-budget. |
| R7 | Legacy v1 persistence remains an accepted compatibility input. |
| R8 | The complete tx-pool-related integration universe remains the P9.8 gate. |
| R9 | Reproducible fixed-binary performance acceptance remains the P10 gate. |

The architecture-adjudication matrix recorded in sections 3, 3.1, 14, 15 and
17 now:

1. prove the exact `develop` race/non-atomic call graphs that require each UAK
   mechanism;
2. account for every new state, lock, log, task, bound and failure domain and
   distinguish risk elimination from bounding or transfer;
3. prove this is the smallest constructively safe model and that valid or
   hostile inputs cannot reach an invariant fault;
4. close every statically derivable correctness, security, liveness,
   publication, identity and complexity issue;
5. re-adjudicate every historical report against current code as
   `confirmed-closed`, `superseded-by-proven-model`,
   `suppressed-with-current-counterevidence` or `open-blocker`;
6. make documentation, machine contracts, isolated tests, full related
   integration coverage and CI selectors agree exactly.

The static architecture gate is therefore closed. P9.8 complete related test
acceptance and P10 profiling/fixed-binary A/B remain independent release
conditions; neither may be inferred from this review.
