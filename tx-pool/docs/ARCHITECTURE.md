# Tx-Pool Architecture: Two-Authority Plan/Apply Kernel

This is the normative design for the current implementation. Executable
behavior and hostile-case evidence live in
[`REVIEW_GUIDE.md`](REVIEW_GUIDE.md); contract maintenance and CI commands live
in [`VALIDATION.md`](VALIDATION.md). Migration notes and checkpoint history are
intentionally not part of the public architecture.

## 1. Decision

The final architecture has two, and only two, executable transaction owners:

1. `TxPool` owns accepted transactions and consensus-facing pool status.
2. `PrePoolKernel` owns retained transactions that have not been accepted.

Every pre-pool transaction is one primary entry in exactly one of six
locations. Queues, wait edges, deadlines, conflict indexes and counters are
derived projections containing identity or accounting only. Accepted mutation
is a read-only `PoolMutationPlan` followed by total `apply_mutation`. Stable
observable effects are appended in the same innermost critical section as
state Apply.

This is deliberately not a universal actor, an undo/rollback engine, a
restart-on-panic system, or a generic workflow framework. It is the smallest
model found that closes the concrete `develop` ownership, RBF atomicity,
liveness and resource-bound failures without serializing read-heavy accepted-
pool or optimistic block-template work.

```mermaid
flowchart TB
    subgraph Ingress["Ingress policy"]
        Remote["Remote"]
        Proposal["Proposal"]
        Recovery["Recovery"]
        Local["Local RPC"]
    end

    subgraph PrePool["Authority 1: PrePoolKernel"]
        Primary["One full-hash primary entry<br/>one of six locations + version + charge"]
        Indexes["Derived identity/accounting indexes<br/>queues, wait edges, deadlines, ready order"]
        Primary -. "derives" .-> Indexes
    end

    Workers["Resolve / verify workers<br/>versioned borrow; no payload ownership"]
    Admission["AdmissionPlan<br/>read-only proof + single-use total Apply<br/>not an owner"]

    subgraph Accepted["Authority 2: TxPool"]
        PoolMap["Accepted PoolMap<br/>membership + graph + Pending/Gap/Proposed"]
    end

    Journal["EffectJournal<br/>committed records; no transaction ownership"]
    Endpoints["Relay / callback / database endpoints"]
    Consumers["RPC / block assembler / persistence readers"]

    Remote --> Primary
    Proposal --> Primary
    Recovery --> Primary
    Primary -->|"lease"| Workers
    Workers -->|"typed settlement"| Primary
    Primary -->|"prepared handoff"| Admission
    Local -->|"direct validation"| Admission
    Admission -->|"total ownership transfer"| PoolMap
    Admission -->|"append with Apply"| Journal
    Journal -->|"after authority locks open"| Endpoints
    PoolMap --> Consumers

    classDef owner fill:#dbeafe,stroke:#1d4ed8,stroke-width:2px;
    class Primary,PoolMap owner;
```

Only the two blue nodes own transaction payloads. Workers borrow one exact
version, `AdmissionPlan` proves a transfer, and the journal owns immutable
effects—not a third copy of transaction lifecycle state.

## 2. Goals and non-goals

The architecture must:

- make retained payload ownership explicit and queryable;
- make stale asynchronous completions harmless through one non-reused version;
- reject all ordinary policy/capacity failures before accepted Apply;
- preserve exact RBF, dependency, reorg, proposal and template liveness;
- bound bytes, entries, active work and graph fan-out before retention/work;
- publish callbacks, relay decisions and diagnostics without state/effect gaps;
- isolate untrusted computation and endpoints using Rust-native boundaries;
- make transaction and authority code panic-free by construction: typed
  outcomes before mutation and a single-consumption total Apply;
- prefer static unrepresentability over runtime validation, and typed results
  over unwind isolation; never use `panic!` plus `catch_unwind` as transaction,
  worker, retry, rollback or authority-control flow;
- retain Local RPC's direct synchronous validation path by design;
- preserve or improve pipeline throughput, subject to controlled A/B.

It does not attempt to:

- make the process survive OOM, abort, FFI corruption or arbitrary memory
  corruption;
- treat an invariant panic as a legal transaction result;
- provide durable mempool persistence across crashes;
- make callbacks or network endpoints exactly-once;
- replace consensus verification, the accepted `PoolMap`, or block-template
  policy with a general scheduler.

## 3. Why `develop` requires structural change

The refactor is justified by failure families, not by a preference for new
types.

| Family | `develop` defect | Required structural boundary |
|---|---|---|
| F1 ownership/ABA | queue pop, active work, orphan retention, clear and re-admit infer location from several structures; stale work can erase or duplicate ownership | one full-hash primary entry and one non-reused version |
| F2 accepted atomicity | RBF/capacity paths can remove accepted transactions before every replacement condition is known | immutable accepted Plan and total Apply |
| F3 stable effects | mutation and relay/callback publication can be separated by saturation or endpoint failure | bounded journal append coupled to Apply |
| F4 resource hostility | queue, orphan, conflict and graph metadata have separate or incomplete accounting | one retained-entry charge plus explicit graph/active bounds |
| F5 dependency liveness | missing/conflicting work can be parked in disconnected mechanisms and miss a wake | one `Wait` owner with exact, level-triggered reverse keys |
| F6 conflict recovery | speculative victim ownership and failed-RBF restoration require ordering-sensitive undo | no speculative removal; conflict history is optional metadata in the same kernel |
| F7 chain/admin ordering | reorg, clear, save and template updates can observe different ownership generations | `TxPool -> PrePoolKernel -> EffectJournal` chain transition plus epoch/reset authority |
| F8 identity/status | raw hash, witness hash, proposal short ID and RPC status can be used as if interchangeable | explicit identity domains and collision-aware projections |

Local hardening of each legacy queue cannot prove the partition invariant: the
bug is the inferred handoff between structures. Conversely, moving accepted
`PoolMap` into one global actor would simplify proof but regress concurrent
read/template/RPC performance and create a single mailbox bottleneck. Two
authorities are the minimum separation that preserves both proof and useful
concurrency.

## 4. Ownership and state

### 4.1 Partition

The single logical ownership state for each full transaction hash `h` is the
closed sum:

```text
Owner(h) = Absent
         | PrePool(location, version, payload)
         | Accepted(status, entry)
```

It is represented by two physical partitions only to preserve useful
concurrency: the accepted `TxPool` remains read-optimized for RPC/template
consumers, while `PrePoolKernel` serializes short lifecycle transitions.  The
representation invariant is:

```text
owners(h) = accepted(h) + prepool(h)
owners(h) is 0 or 1
```

`accepted(h)` is membership in `TxPool`. `prepool(h)` is membership in the
kernel primary map. Physical separation is not permission for two independent
commit authorities. `AdmissionPlan` is the only ordinary cross-partition
transition: it is planned while both generations are stable and applies the
kernel handoff plus accepted insertion as one total operation under the fixed
lock order. Clear/reorg use the separately documented chain-authoritative
generation transition. No other caller may remove one owner and later repair
the other.

### 4.2 Six pre-pool locations

```text
ResolveQueued
ResolveLeased
Wait(Missing | Conflict)
VerifyQueued
VerifyLeased
Ready
```

`Missing` and `Conflict` are reasons inside the one unavailable-work location;
they do not own payloads independently. Recovery is a trusted source, not a
state. There is no persistent `Committing`, `Invalidated`, `RaceLost`,
`RecoveryRetained`, victim hold, undo state or conflict-owned payload.

```mermaid
stateDiagram-v2
    state "Nowhere" as Absent
    state "ResolveQueued" as RQ
    state "ResolveLeased" as RL
    state "Wait(Missing | Conflict)" as Wait
    state "VerifyQueued" as VQ
    state "VerifyLeased" as VL
    state "Ready" as Ready
    state "Accepted in TxPool" as Accepted

    [*] --> Absent
    Absent --> RQ: admit Remote / Proposal / Recovery
    Absent --> Accepted: Local direct Plan / Apply
    RQ --> RL: checkout exact head + version
    RL --> VQ: resolved
    RL --> Wait: exact dependency unavailable
    Wait --> RQ: observed availability level changes
    VQ --> VL: checkout exact head + version
    VL --> Ready: verified + fee gate
    VL --> RQ: snapshot stale
    VL --> Wait: parent unavailable
    Ready --> Accepted: AdmissionPlan + total Apply
    Accepted --> Wait: RBF victim retained as bounded history
    Accepted --> RQ: detached-chain Recovery

    note right of RQ
        Recovery is a source,
        never a seventh state.
    end note
    note right of Wait
        Missing and Conflict are reasons
        inside one owning location.
    end note
    note right of Absent
        reject / expiry / remove / clear
        can terminalize any retained state.
    end note
```

Every worker completion must present the full hash, exact version and expected
leased location. A stale arrow therefore becomes a typed no-op instead of an
implicit state transition.

The entry owns:

- compact raw transaction payload;
- source attribution;
- exactly one typed state payload;
- full-hash version and arrival clocks;
- expiry and conservative retained-byte charge;
- canonical dependency keys.

Derived projections own no transaction payload:

- fair resolve/verify queues;
- proposal-short-ID index;
- peer, parent, waiter, deadline, ready and ready-by-input indexes;
- total/remote/per-peer/conflict residency;
- total/per-owner active work;
- bounded availability epochs and dirty wake keys.

### 4.3 Sources are policy, not location

`PrePoolSource` is `Remote(peer)`, `Proposal`, or `Recovery`.

- Remote retains immutable ingress attribution, current-payload blame,
  declared cycles, remote/per-peer charge and expiry. Trusted same-hash source
  promotion may change scheduling and payload blame, but cannot erase the
  original ingress used for administrative revocation.
- Proposal is trusted and may promote the same-witness Remote entry or replace
  its payload with a trusted witness variant.
- Recovery is trusted detached-chain input and enters the ordinary resolve
  lifecycle parent-first.
- Local never enters the pre-pool. It validates synchronously, commits under
  the same accepted Plan/Apply transaction, and settles any matching retained
  owner. This is an intentional API behavior, not an optimization accident.

## 5. Identity domains

| Identity | Only valid role |
|---|---|
| full transaction hash | primary ownership, accepted membership, lease owner |
| witness hash (`wtx_hash`) | `TxVerificationCacheKey`; verification results cannot alias witness variants |
| proposal short ID | collision-aware index and consensus proposal protocol only |
| entry version (`u128`) | process-global, non-reused ABA token for leases/tickets |
| pipeline epoch | administrative clear/reorg invalidation, not per-entry identity |

A short-ID collision is backpressure/rejection, never proof of duplicate
ownership. Cache accesses construct `TxVerificationCacheKey::from_transaction`.
A stale completion must match full hash, exact version and expected location
before it can mutate the primary.

## 6. State transitions

The public transition family is closed:

| From | Command/outcome | To |
|---|---|---|
| Nowhere | admit Remote/Proposal/Recovery | ResolveQueued |
| retained location | trusted same-hash promotion | same semantic phase or ResolveQueued for a new trusted witness |
| ResolveQueued | checkout exact queue head | ResolveLeased(new version) |
| ResolveLeased | resolved | VerifyQueued(new version) |
| ResolveLeased | exact dependency unavailable | Wait(Missing/Conflict) |
| Wait | all observed keys available | ResolveQueued |
| VerifyQueued | checkout exact queue head | VerifyLeased(new version) |
| VerifyLeased | verified and fee-gated | Ready(new version) |
| VerifyLeased | snapshot/parent stale | ResolveQueued or Wait |
| Ready | selected commit handoff | accepted or bounded Conflict wait/history |
| any retained location | reject/remove/expiry/peer removal/clear | Nowhere |
| any retained generation | chain/admin generation reset | sealed retired generation, then Nowhere/new recovery entries |

Every single-entry transition validates its next entry, exact usage delta,
active-work delta and index constraints before detaching the old projections.
Bounded multi-entry transitions use a private `MutationSet` to derive one
exclusive `PreparedKernelMutation<'_>` containing final primaries, exact
projection changes and reserved monotonic counters. Consuming `commit(self)`
performs the total move. There is no public mutation between Plan and Apply and
no rollback path.

Legal outcomes are typed:

- transaction/policy rejection;
- bounded capacity/backpressure;
- stale lease or location race;
- duplicate/idempotent arrival;
- Apply.

Primary/index/accounting contradictions are not transaction outcomes. Private
state constructors and a prepared authority transaction prevent them on legal
paths; a pre-Apply consistency failure is a typed system fault, never an
assertion inside Apply and never a peer/RPC rejection.

## 7. Scheduling and progress

### 7.1 Fair work queues

Resolve and verify queues contain `WorkKey` identities only. Each source owner
has a queue with a fair turn; runnable heads are derived from global and
per-owner active limits. A large-cycle verify item cannot hide eligible
small-cycle work from a constrained worker.

### 7.2 Ready order

Ready selection uses one total `ReadyKey`, strongest last:

1. source priority: Remote < Proposal < Recovery;
2. exact fee rate by `u128` cross multiplication;
3. absolute fee;
4. earlier arrival;
5. smaller full hash;
6. process-global entry version.

The reverse comparisons for arrival/hash are intentional because the driver
selects the greatest key. Version is globally unique, so a later transaction-
size comparison would be unreachable rather than an additional ordering rule.
There is deliberately no time-dependent aging: source and fee preference are
stable, Remote residency remains deadline/budget bounded, and only bounded
chain-derived Proposal/Recovery ingress can outrank Remote. Under sustained
overload a lower-fee Remote candidate can therefore wait until its residency
deadline, matching fee-market preference at the cost of strict per-candidate
fairness; controlled performance acceptance must measure saturation throughput
and tail latency. Dynamic aging
would require periodic reindexing of every Ready key and is not justified
without evidence that this explicit trade-off is unacceptable.

A `CommitTicket` proves the selected entry's hash, version and rank. A later,
stronger Ready entry does not invalidate an already selected exact ticket; the
single commit driver settles it, then selects the new head. This avoids a legal
arrival race becoming an invariant failure or livelock.

The ticket also carries the immutable ingress peer captured from that exact
Ready version. An expiring, non-evicting peer-ban marker is the revocation
linearization point: new remote admission rechecks it immediately after taking
kernel ownership, and Ready planning rechecks it before building the Accepted
mutation. Marker cardinality is coupled to the network's existing unexpired
ban set; a transaction-residency LRU is deliberately not used because unrelated
newer bans could evict a live fence. The ban path itself removes the indexed
ingress cohort in bounded prepared slices.
Together these edges cover queued admission and Ready-commit races without a
second lifecycle state. A commit already past the final fence remains valid
Accepted state; permitting a later peer ban to roll it back would turn network
administration into a valid-transaction deletion primitive.

### 7.3 Wait is level-triggered

`Wait` records the exact dependency keys and their observed level epochs.
Availability and definitive loss both dirty a bounded key; maintenance drains
bounded slices. A missed notification is harmless because the level remains
visible. Parent terminalization and dependent invalidation share one cohort
Apply, so trusted Proposal/Recovery children re-evaluate terminal policy rather
than parking forever, while Remote children retain their request/expiry policy.
Parent loss demotes resolved/verified consumers using the same exact keys.
Final accepted-pool validation remains authoritative even if background
demotion has not run yet.

## 8. Resource proof

Admission or transition planning accounts conservatively before retention:

- total pre-pool entries and bytes;
- Remote entries and bytes;
- per-peer entries and bytes;
- optional conflict-history entries and bytes;
- total active work and per-peer active work;
- dependencies per entry and dependents per parent;
- Ready inputs and candidates per input;
- accepted pool serialized bytes, retained bytes, cycles and mutation closure;
- effect batches/bytes by trust region;
- candidate uncle count and per-height count.

The charge is held continuously across queued, leased, waiting and Ready
locations. A worker borrows an `Arc`; it does not become a new owner or refund
residency. Optional conflict history degrades to a terminal result when its
partition is full. Graph operations stop at explicit product/fan-out bounds
before mutating.

`NotifyTxs(Vec<TransactionView>)` is a trusted controller boundary. Each item
still passes validation and admission accounting, but the vector itself is not
currently proven by this module to have an upstream length bound; this remains
a documented integration-boundary risk rather than a hidden invariant claim.

## 9. Accepted Plan/Apply

Ordinary admission, RBF and capacity eviction use one concrete
`AdmissionPlan` transaction:

1. Under the accepted pool write guard, calculate RBF conflict closure,
   ancestry, status, candidate parents, capacity evictions and post-state
   totals without mutation.
2. Return every ordinary `Reject` from Plan.
3. While accepted membership is unchanged, build the matching total kernel
   handoff, exact dependency/template receipt and immutable exact-sized effect
   batch. Planning may return a typed stale/capacity result but changes no
   owner, clock, index or budget.
4. The innermost effect-journal predicate first accepts that exact batch, then
   one total Apply installs accepted membership, consumes the kernel owner and
   records the level-triggered template delta before appending the effect. The
   physical order makes the only still-fallible pool insertion the first
   operation; the complete transition remains hidden under both authority
   guards.
5. Release locks, then run callbacks/network/database endpoints.

```mermaid
sequenceDiagram
    autonumber
    participant D as Commit driver (orchestrator only)
    participant P as TxPool
    participant K as PrePoolKernel
    participant J as EffectJournal
    participant E as External endpoints

    D->>P: acquire accepted write guard
    D->>P: build immutable PoolMutationPlan
    alt Reject / Stale / Backpressure
        P-->>D: typed outcome; release guard; original state unchanged
    else accepted plan is complete
        D->>K: acquire kernel and prepare exact versioned handoff
        D->>J: acquire innermost journal guard and test exact batch
        Note over P,J: Fixed nesting TxPool → PrePoolKernel → EffectJournal<br/>No await or foreign endpoint; bounded snapshot reads are Plan-only
        alt exact journal region is Full
            J-->>D: Full before Apply; release journal
            D-->>K: release kernel
            D-->>P: release TxPool
            D->>J: wait for capacity with no state guard
            D->>P: replan from current generations
        else capacity accepted
            D->>P: total accepted PoolMap Apply
            D->>K: consume the prepared pre-pool owner
            Note over P,K: Both authority guards remain held;<br/>no reader observes the physical overlap
            D->>J: append the matching committed effect sequence
            D-->>J: release journal
            D-->>K: release kernel
            D-->>P: release TxPool
            D->>E: publish effects after authority locks open
        end
    end
```

The failure half of the diagram is as important as the success half: no legal
failure occurs after accepted mutation begins, and capacity waiting owns no
reservation or transaction state across the await.

`PoolMutationPlan` contains the candidate, final status, exact removals and
post totals. The enclosing `AdmissionPlan` contains the only matching kernel
handoff and publication receipt. Exclusive prepared borrows prevent the
planned generations from changing; its Apply performs total prevalidated
moves and no assertion or fallible lookup. It cannot discover fee, RBF,
ancestry, identity, effect capacity or resource policy after removing a victim.
Pipeline rejection has its own read-only terminal plan; it likewise cannot
call a fallible park/remove transition after the journal predicate.

This removes the need for nested undo. Rollback is the wrong abstraction here:
it duplicates ownership, index and accounting semantics, and its own failure
becomes a second correctness problem. Read-only Plan plus total Apply proves
the original state is unchanged on every legal failure.

## 10. Cross-authority locking

The nesting order is:

```text
optional serial/work permit
  -> optional EffectJournal capacity wait (no state guard is held)
  -> TxPool read/write
    -> PrePoolKernel mutex
      -> EffectJournal mutex for capacity + total Apply + append
```

No await, callback, mutable database endpoint or retired-generation drop occurs
while the accepted pool/kernel/journal nesting is held. Immutable chain-
snapshot reads are currently allowed only while constructing a bounded Plan.
The normal same-tip final-liveness path consumes the resolved cell's existing
chain provenance as positive evidence after checking the mutable pool overlay,
so it does not repeat the RocksDB point lookup; stale-tip and unproven cells
must revalidate. Remaining RBF/removal snapshot reads are a measured critical-
section cost, not permission to add unbounded I/O. Kernel-only worker
transitions use only the shorter kernel→journal suffix. Code must not acquire
`TxPool` from an effect callback or while already holding the kernel.

### 10.1 Tx-pool-only same-tip cell evidence

`TxPoolResolvedCellChecker` is an admission optimization, not a consensus or
block-validation rule. It may avoid a repeated chain point lookup for one
resolved cell only when all of these conditions hold:

1. the resolved metadata and `pre_resolve_tip` were captured from the same
   `Arc<Snapshot>` used by resolution;
2. `pre_resolve_tip == current_tip_hash`;
3. that cell has `transaction_info`, which proves it came from the chain
   snapshot rather than an accepted-pool producer; and
4. the current accepted-pool overlay was checked first and did not report a
   spender or producer result.

The proof is per cell, never per transaction. A permissive RBF resolve ignores
accepted-pool consumers only; it does not turn a chain miss into metadata, and
pool-produced metadata has no `transaction_info`. A changed tip or absent
chain provenance therefore falls through to the snapshot checker. Removal-
history queries such as `planned_unavailable_parent_hashes` and
`planned_available_dependencies` reason about other transactions and never
consume this evidence. Block, consensus, store and snapshot checkers retain
`CellChecker::is_live_resolved_cell`'s default `is_live(out_point)` behavior.

The first attempt plans optimistically without a worst-case capacity wait. If
the exact batch is `Full`, the caller releases every state lock, waits on that
exact charge as a level-triggered hint, then replans against the new
generations. No reservation crosses an await and no authoritative mutation is
replayed.

## 11. Stable effects

The effect journal has statically partitioned trust ceilings:

- Remote can consume only the Remote ceiling;
- Trusted may use ordinary capacity plus trusted headroom;
- Critical chain/admin work has independent headroom.

Ordinary admission always precomputes one exact immutable batch. `try_apply`
checks its actual charge, executes total state Apply and appends one sequence
while holding the journal mutex. Expensive RBF/capacity/kernel planning never
runs under the journal mutex, and ordinary admission never commits first and
falls back to `GenerationReset` because a post-Apply bound was wrong.

`GenerationReset` remains a deliberately narrower chain/admin mechanism.
Authoritative reorg/clear cannot wait behind callback or relay detail; when its
bounded detail region is saturated, the state transition commits and the
constant-size latest-generation record subsumes that observational detail.
This exception must not be reused by ordinary transaction admission.

The publisher drains FIFO order. Full relayer capacity retains the active head;
individual relay detail may coalesce to `GenerationReset`, which stays pending
until accepted. Callback execution is isolated on a bounded endpoint thread.
Callback, network-ban and recent-reject database calls share a production
timeout and stable per-kind circuit, so one stuck foreign call cannot retain
the sole journal head; endpoint panic cannot unwind state Apply. At most one
timed-out detached call exists per opened blocking endpoint circuit.

An accepted-duplicate `Ok` is also authority-dependent output. Its publisher
holds an accepted-membership read capability through journal append, so clear
or reorg either observes the `Ok` before its reset/removal or wins first and
suppresses the stale acknowledgement. Fresh admission success remains part of
the immutable `AdmissionPlan` batch and has no manual publication gap.

Exactly-once delivery is not claimed. The invariant is: after state Apply, a
bounded stable record exists until detailed publication succeeds or a newer
authoritative generation reset subsumes it.

## 12. Dependencies and conflicts

Three relations are intentionally distinct:

1. causal producers: inputs and expanded dep-group producers;
2. conditional readers: cell/header dep ordering constraints;
3. availability keys: exact causes that keep a pre-pool entry in `Wait`.

Accepted `PoolMap` normalizes out-point memberships into set semantics so the
same input/dep cannot publish or remove one logical edge twice. RBF conflict
closure is bounded by the complete input × candidates product before traversal.
The accepted Plan builds a virtual post-removal overlay for ancestry and
availability; total Apply then moves exactly that closure.

Failed RBF does not remove and restore victims. A verified losing transaction
may remain as bounded `Wait(Conflict)` history only after it has passed the
higher fee gates. When its keys become available it returns through ordinary
resolution and final RBF validation. Optional-history saturation terminalizes
the loser rather than blocking the winning commit. The cohort seal defines the
event cut centrally: unchanged history observes a dependency-level advance,
but a victim retained by that same Apply records the post-Apply level and
cannot treat its own replacement release as a later wake event. This rule adds
no lifecycle state or caller-maintained publication ordering.

## 13. Reorg, clear and persistence

### 13.1 Reorg

The chain-authoritative phase holds the accepted write guard and kernel mutex:

1. reconcile attached/detached blocks and accepted statuses once;
2. collect detached transactions plus the accepted descendant closure of
   detached producers;
3. order one combined recovery set parent-first;
4. compile/apply one bounded trusted kernel recovery plan;
5. if that complete set cannot be represented within the frozen bound, reset
   the ephemeral accepted/pre-pool generation rather than retain an invalid
   parentless suffix;
6. journal the immediate block-assembler reset and optimistic generations.

The recovery payload is an ordinary `PrePoolSource::Recovery` entry in one of
the six locations. No handler-local payload, cross-await `recovery_lock`,
`RecoveryRetained` location or replay retry owner exists. Duplicate attached
raw hashes suppress detached witness variants; cache lookup uses witness hash.

### 13.2 Gap and uncle liveness

Reorg status reconciliation reevaluates Gap entries against the new proposal
window, including Gap→Pending demotion. Block template proposal packaging also
filters candidate uncles whose proposal IDs conflict with proposals that must
be repackaged. A detached block cannot remain an eligible uncle and suppress
the only proposal path of its recovered transactions for an epoch.

Block assembler authority is intentionally asymmetric:

- Reset and `update_full` linearize at one complete-template publication
  boundary while their construction remains concurrent;
- `update_full` has highest priority;
- `update_full` derives uncle content from the bounded candidate authority
  rather than copying a reset template's transient blank projection;
- uncle/proposal/transaction updates remain concurrent, optimistic versioned
  OCC deltas and may be skipped transiently;
- every successful reset/full replacement re-dirties all three partial
  generations, so an acknowledgement racing the replacement cannot erase a
  newer or overwritten level;
- a failed reset remains pending and blocks ordinary deltas until a matching
  full rebuild succeeds;
- zero update interval applies deltas eagerly, retries resets periodically and
  suppresses external miner notification only as configured.

### 13.3 Clear and save

Clear advances the administrative epoch, takes the accepted guard, swaps the
entire pre-pool generation in O(1), installs a generation reset, releases the
guard and only then destroys retired payloads. Stale workers cannot mutate the
new generation because versions never repeat.

Explicit save serializes accepted plus retained recovery-relevant transactions
in dependency order. Graceful shutdown persists only after supervised state
workers and the effect publisher finish normally. An invariant failure marks
persistence ineligible so a corrupt derived state is not written as truth.

## 14. Rust-native failure model

The design does not emulate Erlang supervision or promise recovery from OOM,
abort, FFI faults or memory corruption. It does require panic-free transaction
and authority paths, using Rust ownership and private constructors to separate
expected results from programming contradictions:

```text
type/ownership proof  >  typed pre-mutation Result  >  foreign-code isolation
```

`catch_unwind` is not a correctness mechanism. Internal resolver, verifier,
scheduler, publisher and authority code propagates typed outcomes. Code
supplied by callers is kept outside authority locks and isolated behind a
thread/task/channel boundary; its failure cannot select a transaction state,
rollback, retry or generation transition.

| Class | Rust encoding | Boundary |
|---|---|---|
| malformed/policy | `Reject` / transaction `PrePoolError` | reject/ban according to policy |
| capacity/backpressure | typed error | no mutation; wait/retry or bounded degradation |
| stale/duplicate race | typed stale/duplicate | discard, preserve current owner or retry level |
| shutdown/config/resource | typed external result or startup failure | controlled stop/fail startup |
| resolver/verifier failure | typed job result before settlement | terminalize/quarantine that exact lease; worker continues where safe |
| foreign callback/endpoint failure or hang | thread/task/channel isolation plus timeout/circuit | state remains committed; publisher progresses/coalesces without unwind-driven control flow |
| primary/index/accounting contradiction | typed pre-Apply system fault | controlled stop, skip persistence; never blame input |

Legal hostile input must not reach the last row or unwind the service. The
prepared transaction owns an exclusive authority borrow until `commit(self)`,
and state-specific constructors carry the facts needed by total Apply. Adding
a recover-and-repair path after partial Apply would still expand the state
machine and risk persisting corruption, so contradictions are detected before
the first mutation rather than asserted or repaired afterward.

The remaining operational consequence is explicit: a pre-Apply system fault
stops the tx-pool without persisting the generation. The defense against a
repeatable peer DoS is structural—peer-selectable parsing, policy, capacity,
stale and endpoint outcomes are typed, while total Apply has no assertion
boundary—not a restart of unknown authority state.

## 15. Block-template authority

Accepted status is consensus-facing and remains `Pending`, `Gap` or `Proposed`.
RPC compatibility may project Gap as pending, so review/tests must inspect
internal detail when liveness depends on the distinction. Proposal selection
iterates true Pending; transaction selection commits true Proposed.

Template state has two forms:

- a full authoritative snapshot generation, updated by Reset/`update_full`;
- coalesced uncle/proposal/transaction dirty generations applied concurrently
  and optimistically through version checks.

```mermaid
flowchart TB
    ResetEvent["Chain/admin Reset<br/>generation-tagged snapshot"] --> ResetBuild["Build reset template<br/>without publication guard"]
    FullEvent["High-priority full reconcile"] --> FullBuild["Build full template<br/>from TxPool + candidate-uncle authority"]

    UncleDirty["Uncle dirty generation"] --> UncleBuild["Build uncle delta<br/>and refresh proposals"]
    ProposalDirty["Proposal dirty generation"] --> ProposalBuild["Build proposal delta"]
    TxDirty["Transaction dirty generation"] --> TxBuild["Build transaction delta"]

    subgraph Publish["CurrentTemplate publication boundary (short write guard)"]
        ResetApply["Reset Apply<br/>exact reset token; advance reset_epoch"]
        FullApply["Full Apply<br/>same reset_epoch; ignores partial revision"]
        PartialApply["Partial Apply<br/>CAS captured template revision"]
        Current["CurrentTemplate<br/>template + size + revision + reset_epoch"]
        ResetApply --> Current
        FullApply --> Current
        PartialApply --> Current
    end

    ResetBuild --> ResetApply
    FullBuild --> FullApply
    UncleBuild --> PartialApply
    ProposalBuild --> PartialApply
    TxBuild --> PartialApply

    ResetApply -->|"re-dirty all partial levels"| Reconcile["Uncle + proposal + transaction reconcile"]
    FullApply -->|"re-dirty all partial levels"| Reconcile
    Reconcile --> UncleDirty
    Reconcile --> ProposalDirty
    Reconcile --> TxDirty

    classDef authority fill:#dbeafe,stroke:#1d4ed8,stroke-width:2px;
    class Current authority;
```

Reset and full construction may overlap partial construction; only publication
is serialized. A reset invalidates an older full build through `reset_epoch`.
A full build wins over intervening partial revisions, while each partial update
must match its captured `TemplateRevision`. Re-dirtying all three partial
levels after reset/full closes the lost-acknowledgement race without routing
all work through one actor.

Candidate-uncle retention is the input authority for both full and uncle-only
plans. Preparation clones the bounded cache under a short synchronous lock;
chain lookups and template construction run after that lock opens, and stale
cleanup is committed only with the matching successful publication token.

Candidate uncles are bounded at production limits (128 total, 10 per height)
in both production and tests. An uncle is removed/rejected when it is on the
main chain, already embedded, epoch/target-invalid, structurally invalid, or
conflicts with proposal liveness. Test builds do not change these limits.

## 16. Performance model

Correctness structure is also the intended performance structure:

- one short synchronous kernel mutex section per retained transition;
- identity-only fair queues and reverse indexes;
- `Arc` borrowing for raw/resolved/verified payloads;
- stack-sized plans for single-entry transitions;
- heap-backed cohort planning only for bounded multi-entry changes;
- move-only Apply; no cloned old-entry snapshot/undo journal;
- one serialized commit driver instead of competing commit owners;
- accepted reads remain behind the existing `RwLock`, not a global actor;
- no foreign/mutable I/O or payload destruction under authority locks;
- same-tip resolved chain provenance removes normal duplicate liveness reads;
  remaining bounded immutable-snapshot Plan reads require dedicated lock-hold
  measurements before any cache, prefetch or optimistic-retry protocol is added;
- block-template/effect notifications coalesce level-triggered work.

Performance acceptance is empirical, not inferred from line count. The final
gate compares clean revisions with controlled warmup/cache conditions and
measures throughput, tail latency, RSS/allocation, lock hold time, worker
utilization, commit, reorg and template latency. A material regression blocks
production even if correctness tests pass.

## 17. Extensibility rules

A new stage or policy is acceptable only if it answers these questions:

1. Does it require retaining a payload, or can it be derived metadata/work?
2. Which of the two authorities owns it at every instant?
3. Can it be encoded inside an existing state payload/reason/source rather
   than a seventh location?
4. What exact byte, entry, active and graph bounds apply before retention?
5. What versioned command consumes it, and what are all legal outcomes?
6. Does it alter accepted membership? If so, where is immutable Plan and total
   Apply?
7. Which stable effects must be appended with Apply?
8. What level-triggered condition guarantees progress after a lost wake?
9. Which review behavior and unit/process regression prove the boundary?
10. What benchmark demonstrates no material hot-path regression?

A proposal that adds a payload-owning side cache, rollback journal, broad
retry, reverse lock order, unbounded vector/graph, or test-only production
behavior is rejected at design review.

## 18. Why this is the preferred architecture

### Versus hardening `develop`

Per-queue locks/checks cannot make a transaction's location single-valued
across pop, await, cancellation, conflict parking and clear. Fixes remain
ordering-dependent and every new queue adds another handoff proof. The kernel
removes the root inferred-location premise.

### Versus a universal tx-pool actor

One actor could serialize all ownership but would route accepted reads,
template queries, RPC inspection and graph computation through a mailbox. It
reduces concurrency, raises tail latency/backpressure coupling and makes large
messages part of the trusted scheduling surface. The two-authority design
serializes only mutation boundaries that require atomicity.

### Versus nested undo/transactions

Undo snapshots duplicate state and require a second correct implementation of
indexes, budgets, victims and wakeups. Apply failure then needs rollback failure
semantics. Plan/Apply preserves the original state until all ordinary failure
conditions are exhausted and has one mutation implementation.

### Versus persistent commit/conflict/recovery states

`Committing`, `RaceLost`, victim holds and `RecoveryRetained` encode protocol
execution as durable ownership. They enlarge the partition, persistence,
expiry, clear and ABA matrices. A single commit driver, bounded `Wait` reason
and ordinary Recovery source provide the required semantics without those
locations.

### Versus invariant-repair/restart

Repairing an authority after a contradiction requires trusting projections
already proven inconsistent and choosing which externally visible effects to
replay. Rust isolation remains appropriate at genuinely foreign computation
and endpoint boundaries. Authority transitions instead use proof-carrying
state and typed pre-Apply faults; no assertion or unwind is part of the
transaction protocol. This is simpler than repair because hostile legal
outcomes are fully classified before Apply and partial mutation is
unrepresentable.

## 19. Proof obligations

The machine contract is [`architecture-contract.json`](../architecture-contract.json).
The human proof obligations are the stable, independently reviewable leaves
below.  For reasoning they form eight broader theorem families: partition
(T1), lease causality (T2), budget conservation (T3), derived views (T4--T5),
linearization (T6--T7 and T9), bounded hostility (T8), progress (T10--T11),
and accepted/template consistency (T12--T13).  The leaf IDs are deliberately
not merged or renumbered: grouping
shortens the proof narrative, while keeping the leaves preserves precise test
anchors and prevents a passing sibling clause from hiding a zero-match one.

The leaves are:

- T1 Partition: accepted and pre-pool ownership are disjoint and unique.
- T2 Lease: only exact full-hash/version/location work mutates a primary.
- T3 Budget: retained ownership iff it is charged; all fan-out/work is bounded.
- T4 WaitExactness: Wait owns exact observed dependency causes.
- T5 ReadyExactness: Ready ranks/inputs derive from its primary payload.
- T6 AtomicAcceptance: every ordinary failure precedes accepted Apply.
- T7 StableEffects: successful Apply leaves a bounded publishable record.
- T8 BoundedHostility: peer input cannot cause unbounded residency/CPU/fan-out.
- T9 ChainSerialization: reorg/clear/save observe one authority order.
- T10 CriticalSchedulability: Remote saturation cannot consume chain headroom.
- T11 LevelTriggeredProgress: lost wake edges do not strand executable work.
- T12 AcceptedStatusExactness: Pending/Gap/Proposed transitions match snapshot.
- T13 TemplateAuthority: reset/full/deltas cannot publish an old-parent or
  proposal-stranding template.

The T1–T13 mapping in [`review-behaviors.json`](../review-behaviors.json) is the
current executable evidence. Development-era finding numbers are intentionally
not part of the normative model.

## 20. Implementation map

The model is distributed by responsibility, not by lifecycle ownership:

| Responsibility | Primary implementation | Review boundary |
|---|---|---|
| Pre-pool ownership, state, leases, budgets and indexes | `src/component/pre_pool/` | T1–T5, T8, T10–T11 |
| Accepted membership, graph and mutation plans | `src/pool.rs`, `src/pool_map/` | T6, T12 |
| Cross-authority admission and administrative transitions | `src/service/pipeline_ops.rs`, `src/process/submit/` | T6–T10 |
| Resolve, verify and commit workers | `src/service/stages/`, `src/service/workers.rs` | T2, T10–T11 |
| Stable effects and external endpoint isolation | `src/service/effects.rs` | T7, T10 |
| Reorg, clear and persistence | `src/process/reorg.rs`, `src/process/mod.rs`, `src/persisted.rs` | T9, T12 |
| Block-template reset/full/OCC deltas | `src/block_assembler/` | T13 |
| RPC/network dispatch and source policy | `src/service/controller.rs`, `src/service/dispatch.rs`, `src/service/message.rs` | T7, T12 |

`TxPool` and `PrePoolKernel` remain the only payload authorities. Modules in
this table coordinate or project those authorities; their existence does not
create another owner.

## 21. Residual risks

A residual is an explicit boundary, not an implicit bug waiver. `Eliminate`
means a focused change can remove it without changing the ownership model;
`Mitigate` means the model can bound but not erase it; `Accept` means removing
it would add a disproportionate owner/protocol; `Validate` means evidence, not
another mechanism, closes it.

| ID | Disposition | Current boundary | Closure rule |
|---|---|---|---|
| R1 | Mitigate | A genuine pre-Apply primary/projection contradiction is a typed system fault: service stops and the generation is not persisted. | Legal input must remain excluded by private types, narrow result domains and total Apply. A reachable legal path requires redesigning that operation-specific API, never catch/repair control flow. |
| R2 | Accept | Callback/network detail is bounded and reconcilable, not exactly-once. | Exactly-once would require a durable transactional outbox and idempotent receivers; do not add that protocol without a product requirement. |
| R3 | Mitigate | A timed-out blocking endpoint may leave one detached call per endpoint kind until it returns; stable circuits suppress later calls. | Keep endpoint-kind cardinality bounded and authority locks released. Prefer cancellable async APIs when available. |
| R4 | Accept | Explicit pool persistence is neither a crash-durable WAL nor a cancellable filesystem transaction; shutdown I/O may delay exit. | Keep I/O outside transaction authority. Add timeout/metrics only for an evidenced shutdown SLO; do not move I/O under authority locks. |
| R5 | Accept | OOM, allocator abort and process corruption are outside in-process recovery. | Process supervision and restart own this boundary. |
| R6 | Eliminate | The trusted `NotifyTxs(Vec<_>)` controller path has bounded elements but no locally proven outer batch bound. | Introduce a bounded ingress type or bounded slicing at the source before claiming a complete hostile-memory proof. |
| R7 | Improve with evidence | `TxSelector` stops after 4,000 consecutive non-fitting packages, bounding CPU while permitting bounded template underfill. | Change only through a resumable cursor or fit-aware index with packing-quality and CPU/RSS A/B evidence; removing the cap is invalid. |
| R8 | Eliminate | Kernel residency, rejection classes, worker failure and effect-region usage lack dedicated production metrics. | Add low-cost counters/gauges before claiming automated operational SLO detection. |
| R9 | Accept for compatibility | A legacy or hand-authored v1 persistence file may order a child before an expanded dep-group parent and lose that local mempool child during serial replay. | A future fix must be a versioned batch-resolve/retry loader, not another raw ordering heuristic. Chain state is unaffected. |
| R10 | Accept | Raw `Wait(Conflict)` cannot know expanded dep-group, header context or maturity until re-resolution. | Accepted/verified victims retain exact expanded edges. Do not retain another verified owner or contextual wake protocol without a concrete liveness counterexample. |
| R11 | Eliminate | A configured non-terminating block-template notify script is timeout-bounded at the Rust task, but child-process termination is not proven. | Specify cross-platform kill-on-drop/process-group semantics and bound configured endpoint count. |
| R12 | Validate | Throughput, tail latency, CPU, allocation/RSS and lock-hold superiority are not established by correctness tests. | Pass the clean, repeated, fingerprint-matched A/B protocol in [`BENCHMARK.md`](BENCHMARK.md). |

## 22. Release conditions

A release or superiority claim applies to the tested revision only. It
requires:

1. all read-only contract, documentation and test-layout validators in
   [`VALIDATION.md`](VALIDATION.md) to pass without generated drift;
2. the complete `ckb-tx-pool` internal-feature nextest suite and strict clippy
   gate to pass;
3. the complete managed integration impact inventory to agree with
   `ckb-test --list-specs` and pass through `make integration` without filtering
   failures;
4. every changed behavior to retain its focused unit and process evidence in
   [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md); and
5. the controlled performance gate R12 to pass before claiming that the
   redesign is performance-neutral or superior.

Accepted or mitigated residuals remain visible in this document. New evidence
must narrow a residual or add a new current ID; it must not revive a historical
ledger or hide risk behind another owner, repair protocol or retry state.
