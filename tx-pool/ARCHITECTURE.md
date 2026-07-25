# Tx-Pool Architecture

Status: independently audited **GO with recorded non-blocking risks**;
implementation is authorized. Measured performance acceptance remains open.

This is the permanent normative architecture contract promoted at P0 after the
independent audit in [`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md). It
explains why the target is necessary and superior to `develop`, defines the
finite proof surface, and records rejected alternatives and residual risks.
[`REVIEW_GUIDE.md`](REVIEW_GUIDE.md) is the behavior-driven review entry point;
[`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) controls the migration;
[`pipeline.md`](pipeline.md) remains the historical migration log. If those
documents disagree about target semantics, this file is authoritative.

## 1. Decision rule

The refactor is valuable only if it is demonstrably necessary and superior to
`develop`. Passing tests is not sufficient. The design must establish all of:

1. **Necessity**: every retained mechanism closes a concrete `develop`
   counterexample that a smaller design cannot close.
2. **Safety superiority**: legal input cannot reach process/service fail-stop,
   ownership loss, partial accepted-pool mutation, or unbounded work/state.
3. **Minimality**: one mechanism closes each root-cause family. A state, index,
   token, queue, undo protocol, or recovery store without an independent
   counterexample is removed.
4. **Static performance superiority**: independent resolve/verify concurrency
   is retained; normal work is bounded by transaction edges plus a configured
   local cohort; no population-sized hot-path scan or additional long-held lock
   is introduced.
5. **Reviewability**: the authoritative state, transition table, resource
   equations, lock order, linearization points, and recovery domains form a
   finite proof surface.

If these claims cannot be established, the current refactor must not merge.
Benchmark evidence is a later, explicitly authorized gate and cannot rescue a
design that fails the structural proof.

## 2. Verified baseline

`develop` has three independently locked executable stores: accepted
`TxPool`, `VerifyQueue`, and `OrphanPool`. A verification worker removes a
payload from `VerifyQueue` before work completes. RBF removes accepted victims
and emits callbacks before every later insertion/revalidation/limit step has
succeeded. Failed-replacement recovery is spawned and ignores a later queue
admission error.

The current checkpoint replaces those stores with a coordinator, but current
production source is about 24.2k Rust lines excluding tests and benchmark,
versus about 7.3k on `develop`. The largest new proof surfaces are coordinator
lifecycle/capacity/conflict/undo code, runtime adapters, the conflict cache, and
effect publication. This delta must be justified mechanism by mechanism; test
growth is accounted separately.

## 3. First external design report: verified and corrected

The report `Tx-Pool Pipeline — Verified Design Specification` is useful as a
proof that the `develop` multi-store ownership model must be left behind. It is
not sufficient to freeze the current HEAD topology.

| Report claim | Verification | Design consequence |
|---|---|---|
| D1-D8 identify real structural defects in `develop` | Substantially correct | Retain one pre-accept ownership oracle, versioned worker leases, final pool authority, bounded resources, and post-state effects |
| A third executable payload store is unsafe | Correct | Payload queues must be derived scheduling indexes, never owners |
| `ConflictCache` is merely non-executable history | Incomplete | It retains transactions and later transfers them into the coordinator, so it is a dormant pre-accept owner; unification with the kernel wait state must be evaluated |
| HEAD's dual `(incarnation, revision)` tokens must remain | Not proven | One globally monotonic entry version prevents both re-admit and within-admission ABA; retain two tokens only if a counterexample defeats one token |
| `Committing` and `Invalidated` must remain | Not proven | A single commit driver plus peek/plan/final-CAS can avoid `Committing`; final pool revalidation plus wait demotion can avoid invalidation cascades |
| Conflict relation counts and victim indexes are required for bounded work | Not proven | A global ready order plus per-input ordered buckets, and reject-on-full admission with trusted reserve, may provide stronger bounds with fewer projections |
| HEAD already realizes `Plan -> Apply -> Publish` | Only partially true | Generic undo and fallible post-mutation handoff show that Apply is not yet total; the redesign must move every ordinary failure before mutation |
| Legal input does not reach invariant fail-stop | False on the reviewed checkpoint | A valid duplicate expanded dependency reached a PoolMap removal assertion and Authoritative fail-stop; this is an explicit topology-unfreeze falsifier |
| `effect reserve -> recovery_lock -> TxPool` is the lock order | Incorrect for reorg/clear | The verified deadlock-free order is `recovery_lock -> effect credit -> TxPool`; ordinary commits do not acquire `recovery_lock` |
| Correctness superiority is enough to freeze HEAD | Does not meet the requested gate | Complexity, availability, maintainability, and static performance are part of the required superiority proof |

The report therefore contributes evidence for leaving `develop`, but its
recommendation to stop redesigning is rejected.

## 4. Root-cause families and minimum necessary mechanisms

| Family | Concrete counterexample | Minimum mechanism | Mechanisms not yet justified |
|---|---|---|---|
| F1 ownership/ABA | queue pop followed by panic, cancellation, removal, clear, or re-admit | one pre-accept entry map; worker borrows with a versioned lease; queues contain IDs only | multiple queue owners, `ActiveSet`, dual entry tokens |
| F2 accepted-pool atomicity | `develop` removes RBF victims before revalidation, insertion, and size limiting | immutable `PoolMutationPlan` under the pool write guard; validate then total apply | general rollback journal, nested/cohort undo, post-mutation coordinator failure |
| F3 effects | callback/channel wait interleaves with state mutation or is lost after cancellation | bounded ordered effect journal with capacity reserved before mutation | effects encoded as lifecycle states or callbacks under locks |
| F4 resource/CPU DoS | queued, active, waiting, metadata, or backing allocation is not continuously bounded | exact entry charge; global/remote/per-peer quotas; fair fixed-worker scheduler; bounded local cohorts | eviction of unrelated pre-pool owners on every admission, victim relation graphs |
| F5 dependency liveness | a child waits on only one of several parents, or loses an expanded dep-group wake edge | one waiting state and reverse `DependencyKey -> entry` index; every retry re-resolves current state | ordered-resolve lifecycle, transitive invalidation state/cascade |
| F6 conflict recovery | removed accepted transactions are dropped or reactivated before every blocker is available | conflict waiting owned by the same pre-accept map; availability events only requeue for authoritative resolve | separate transferable `ConflictCache`, recovery generations/channels |
| F7 chain/admin ordering | save/clear observes a partial reorg; template loses an authoritative full/reset | charged/persistable `RecoveryRetained` generation plus existing assembler priority contract | a lock held across replay awaits or a new global actor |
| F8 identity/accepted graph | short-ID collision, witness-cache alias, duplicate derived edge, stale uncle proposal exclusion | full tx hash at ownership boundaries, typed witness cache key, canonical edge sets, proposal-wins optional-uncle filter | pipeline lifecycle states |

F1-F7 justify a structural change from `develop`. They do not justify the
current number of states and projections. F8 is orthogonal correctness work and
must not be used to inflate the pipeline's necessity claim.

## 5. Candidate architectures

### A. Patch `develop` while retaining its stores

Rejected unless a new construction proves otherwise. A side registry added to
make queue/orphan/active ownership atomic becomes the pre-accept kernel in all
but name. Without it, membership and resource invariants remain distributed.

### B. Keep the current coordinator topology

Not accepted as the default. It closes many `develop` defects but currently
requires persistent `Committing`/`Invalidated`, two entry tokens, four ticket
queues, conflict relation counts, two victim indexes, a separate conflict
owner, generic undo, and service fail-stop. The valid-input fail-stop
counterexample disproves its current safety-superiority claim.

### C. One actor or one `RwLock` for accepted and pre-accept state

Rejected by the frozen design on the static performance proof. Resolver operations
need concurrent read access to accepted pool cells while short pre-accept
transitions continue. A fair combined `RwLock` lets queued writers block later
readers and serializes independent resolves; an actor must either block on
resolve/verify work or introduce snapshots/CAS that reproduce a more complex
two-authority protocol.

### D. Thin Transaction Kernel plus asynchronous shell

Frozen design candidate; section 11 records how its design obligations close.
It keeps exactly
two executable authorities for a performance reason:

- `TxPool`: accepted membership and its graph, behind the existing async
  `RwLock` so independent resolvers and template/RPC readers remain concurrent.
- `PrePoolKernel`: all unaccepted payloads, behind one short-held synchronous
  mutex. Workers and queues only borrow or index these entries.

The universal nested order is `TxPool -> PrePoolKernel`. The kernel never
acquires or awaits `TxPool`.

## 6. Target pre-accept state specification

In this document `full_hash` means the complete 32-byte raw transaction hash,
as opposed to `ProposalShortId`; it does not include witnesses. Witness-bearing
validation identity is always the separately typed `wtx_hash`. Different
witness variants therefore share one lifecycle owner but never one verification
cache entry.

The one normative state encoding is section 21.2's seven-variant enum. Earlier
drafts used a second nested `Resolve { run }` / `Verify { run }` notation here;
it is deleted because two representations of one state machine create review
drift even when they are mathematically equivalent.

Common entry fields hold full transaction identity, retained ingress
attribution (`Remote(peer)` or `Proposal`),
deadline, global monotonic version, and compact dependency keys. There is no
persistent `Committing`, `Invalidated`, `RaceLost`, or conflict-recheck state.
`RecoveryRetained` is the charged, persistable per-transaction location of a
detached-reorg item before it enters the ordinary Resolve phase; it replaces an
out-of-state replay vector and a lock held across awaits. There is one resolve
stage: ordinary remote input and historical recovery run the same validation
logic. Current `RawStage::{PreCheck, Resolve}` values select two queues that
execute the same resolve function and are not sequential business phases.

Source promotion changes attribution and scheduling. Witness replacement is a
new version and makes an old lease stale. A commit plan captures both version
and ready rank, so source/rank changes cannot validate a stale plan without
forcing active worker churn.

## 7. Derived indexes and scheduling specification

- Full hash is the entry identity. The accepted primary map is full-hash keyed;
  the protocol short-ID index owns at most one full hash because proposal and
  compact-block lookups expose one slot. A distinct colliding hash receives
  typed retryable namespace backpressure, never duplicate success or aliasing.
- Waiting reverse index: `DependencyKey -> bounded set<tx_hash>`.
- Ready global order: exact `BTreeSet<ReadyKey>` using the single order frozen
  below.
- Ready conflict index: `OutPoint -> BTreeSet<ReadyKey>`; no relation degree or
  stronger-count projection.
- Resolve/verify fairness: exact per-owner queues plus an ordered set of
  runnable owner heads. Remote owners are round-robin/fair; trusted/proposal
  work has reserved capacity and priority. Fixed worker counts bound global
  active work; an owner is omitted from runnable heads while at its active cap.
- Entry charge is computed from its closed state and compact vector lengths,
  not stored as several mutually dependent cached charge fields.

`ReadyKey` is not an implementation detail. Greater keys win and the exact
order is:

```text
ReadyKey = (
    source_class,                 // Remote < Proposal; Local is direct
    fee / serialized_size,        // compared by u128 cross multiplication
    absolute_fee,
    reverse(arrival_sequence),    // older wins
    reverse(full_hash),           // smaller stable identity wins
    entry_version,
)
```

`serialized_size` includes the current witness variant and is non-zero.
`entry_version` makes equality exact if a trusted witness replacement changes
size under the same raw hash; only one version can be indexed at a time. Source
class is scheduling priority, not an RBF verdict: final pool planning still
applies every replacement fee rule under the write guard. Proposal priority is
retained for proposal-window liveness; Local submission never waits behind this
order because it executes synchronously under the same commit serial permit.
There is no `committing` bit. A Ready owner has one residence deadline which a
stronger arrival cannot extend. Continuous stronger/proposal traffic may keep a
weaker candidate from acceptance, but can only end it in commit, typed rejection,
dependency wait, or bounded expiry; the API does not promise acceptance under
an infinite higher-priority workload.

Remote admission does not evict unrelated pre-pool entries. It returns typed
backpressure when its global or per-peer maximum is full.
Proposal/trusted
liveness comes from reserved quota, not a general victim transaction. This is
expected to delete capacity victim indexes and most multi-entry undo paths; it
relies on the partition/fairness proof below to keep attacker-filled remote or
conflict-history quota from suppressing proposal/chain-recovery progress.

### 7.1 Why fixed partitions replace pre-pool victim selection

The `develop` verify queue already rejects on a fixed byte bound and gives
proposal work scheduling priority; it does not economically evict one remote
raw transaction for another. The current coordinator's global capacity victim
order also permits displacement by higher **source trust**, not by an
unresolved remote transaction's claimed fee. Fee is unknowable until accepted
inputs are resolved. Therefore retaining global victim indexes does not solve
same-trust remote economic pinning; it mainly implements trust isolation with
multi-entry mutation and undo.

The target encodes that isolation constructively:

- Remote has a hard global byte/entry ceiling plus per-peer ceilings and fair
  runnable owner heads. It cannot consume Proposal, ConflictHistory or
  ChainRecovery reserve.
- Proposal may use its reserve and otherwise borrow currently unused Remote
  capacity, but borrowed bytes are reclaimable only by completing/expiring the
  proposal itself, never by evicting an unrelated owner during admission.
- ConflictHistory has a small optional bound for retained RBF victims; overflow
  is terminalized deterministically. ChainRecovery has a separate authoritative
  retained-batch bound which no other class can consume. An over-bound chain
  event uses the generation-swap rule rather than asking remote or conflict
  work to absorb unbounded replay.
- Local direct submission is caller-owned synchronous work under a trusted
  scratch/worker permit and does not reside in a pre-pool partition.

Within Remote, saturation is load shedding, not a false promise of fee
priority. A peer receives bounded fair work share after admission; verified work reaches the single
commit driver without an extra capacity-victim race. Once accepted, the ordinary
PoolMap fee/CPFP size policy supplies economic eviction. This is stronger and
more falsifiable than the current raw global policy, while deleting the
capacity-victim index, protected sets and their undo protocol.

This boundary is explicit: an attacker with at least
`ceil(remote_global_limit / remote_per_peer_limit)` simultaneously attributed
identities can keep Remote admission saturated by refilling expired work. No
tx-pool-only policy can promise an honest remote slot under unlimited connection
Sybil identities, and fee is unknowable before resolve. The maximum retained
cost and residence of every attempt remain bounded, Proposal/Local/chain
progress is isolated, and relay retry is the recovery mechanism. A future
per-connected-peer reserve is rejected unless the network supplies an atomic
session-lifecycle/fencing contract; otherwise disconnect churn and late
messages merely move the pinning bug into a new slot owner.

Configuration must prove that Proposal holds at least one maximum supported
proposal ingress item/batch and ChainRecovery holds one maximum retained chain
batch; total memory includes borrowed usage. Proposal sizing is an operational
liveness bound, not the whole consensus proposal window: raw transactions are
fetched asynchronously and failure to retain every advertised proposal cannot
block chain acceptance. Saturation, peer churn, promotion, borrow/release and
proposal/chain-recovery progress are mandatory adversarial tests.

## 8. Planned accepted-pool commit

One serial asynchronous Ready commit driver performs:

1. briefly take the kernel, clone the current best Ready candidate and its
   `(version, rank)`, then release the kernel;
2. take `TxPool` write;
3. build a read-only `PoolMutationPlan` against the locked pool and snapshot,
   including the exact immutable effect batch;
4. take the kernel and prepare its bounded winner/loser/wait delta;
5. reject the plan if the entry/version/rank changed or a stronger direct
   conflict is now at an input bucket head;
6. validate touched derived memberships, pre-reserve containers, and verify
   that the kernel-owned effect journal's ordinary/critical static partition
   can hold the exact batch;
7. apply pool and kernel plans with no ordinary error return;
8. append the batch inside the same kernel critical section;
9. release locks and publish effects asynchronously.

Local submission remains caller-driven and synchronous by design. It performs
the same bounded validation and `PoolMutationPlan` construction, then shares
the same commit/plan serial permit and final `TxPool -> kernel` Apply; it does
not enter an asynchronous pre-pool state merely to reuse the driver. The paired
kernel delta settles any same-hash owner and records availability loss for
staged direct conflicts, so Local cannot create dual ownership or bypass final
dependency/RBF/capacity rules.

The pool plan contains the complete deduplicated removal union for RBF and size
eviction. One configured physical displacement cap covers the union. RBF
rules, current dependency liveness, proposal status, true-causal ancestor
constraints, and final capacity are checked against a virtual pool view
excluding that union. No victim is removed merely to discover that a later
condition fails. Cell-reference ordering never authorizes admission-time
displacement; section 12.3 proves why it is a template-ordering relation rather
than accepted-pool ancestry.

This construction removes generic rollback: every transaction/policy/capacity
failure occurs before step 8. Allocator abort remains process-level; logical
invariant drift is handled as a recovery condition before mutation.

## 9. Failure containment specification

Legal input has only typed success, stale, waiting, capacity, or rejection
outcomes. No transaction-shaped condition may call `assert!`, `expect`, panic,
or service fail-stop.

Derived-index drift is handled before apply. The escape path is one cohesive
`DefectDomain`, not independent quarantine/cooldown/circuit mechanisms:

```text
DefectGate = Closed
           | Cooling { until, bounded_wtx_fingerprints }
           | Open { bounded_wtx_fingerprints }

DefectDomain = { spare_generation, disposal_permit, gate, reset_counter }
```

The gate applies only to untrusted Remote ingress. Local, Proposal, chain,
query and assembler authority remain independently scheduled. A witness
fingerprint is diagnostic/loop suppression data, not another transaction
owner. The one prebuilt empty spare and one `DisposalPermit` keep the old
generation charged until it is dropped outside authority locks; a replacement
spare is built only after disposal releases its permit.

The response order is:

1. reconstruct the bounded touched projection and retry once;
2. if the pre-pool kernel would need a population rebuild, swap out that
   bounded entry/index generation, settle it with `GenerationReset`, record the
   triggering witness fingerprint, and continue;
3. for accepted-pool drift, attempt one observable full projection rebuild from
   primary entries under the write guard; on recurrence/allocation failure skip
   persistence for that generation, reset the ephemeral mempool and keep the
   node service alive;
4. repeated recovery moves the one gate from Closed to Cooling and eventually
   Open instead of creating a crash/reset loop.

Worker panic invalidates or requeues only its versioned lease. Callback panic is
contained by the effect publisher. Chain state is never part of this recovery
domain.

Every external command/worker boundary catches unwind outside authority locks.
A Plan/worker panic is mutation-free: it releases permits, records the bounded
witness fingerprint and advances the Remote gate when applicable. Only an
unwind from the tiny total-Apply closure requires the locked generation swap.
This separation prevents a malformed-input bug in read-only code from causing
an unnecessary accepted-pool reset while still avoiding service-wide fail-stop.

## 10. Constructive invariants

1. **Partition**: for every full transaction hash, accepted membership XOR one
   pre-pool entry; a worker lease is a borrower, not a location.
2. **Lease**: completion mutates state iff hash, version, and expected stage
   match.
3. **Budget**: aggregate charge equals the sum of closed entry charges,
   explicitly bounded derived-index/effect charges, and every active borrower
   permit. Removing an owner cannot leave a worker-held payload `Arc`
   uncharged.
4. **Wait exactness**: every Wait key has one reverse membership and only Wait
   entries have such memberships.
5. **Ready exactness**: every Ready entry owns one global rank key and one key
   in each canonical input bucket; no other entry does.
6. **Atomic acceptance**: no fallible operation follows the paired pool/kernel
   apply linearization point.
7. **Stable effects**: every externally required outcome is exactly charged
   and admitted to a static journal partition before Apply, then appended
   before locks open; I/O observes committed effects only.
8. **Bounded hostility**: normal adversarial work is `O(tx edges + local cohort
   cap)` and retained memory is covered by global/trusted/remote/per-peer bounds.
9. **Chain serialization**: every paired commit/reorg/clear/save boundary uses
   `TxPool -> kernel`, never holds either lock across await, and persistence
   includes charged `RecoveryRetained` plus active recovery-source entries.
10. **Critical schedulability**: ordinary effect storage, a hung endpoint or an
    older optimistic assembler update cannot delay/apply after newer
    chain-critical authority; sequence/generation checks make bypass safe.
11. **Level-triggered progress**: every eligible queued/Ready owner has an
    exact runnable head or a bounded permit/capacity reason; notifications are
    hints and losing one cannot strand ownership. A Wait owner has at least one
    current unsatisfied key and a monotonic epoch path back to Resolve.
12. **Accepted status exactness**: every accepted `Gap`/`Proposed` status is
    justified by the current snapshot window; leaving both windows demotes to
    `Pending`, whose proposal ID is eligible for normal template packaging.
13. **Template authority**: a returned template matches the current
    `(chain_generation, parent_hash)`; reset/full notifications are hints and
    full remains the highest-priority same-generation authority.

These invariants will be checked against a simple recomputing reference model
after every generated command. Targeted integration tests remain necessary for
lock/wakeup/cancellation, reorg, block assembler, RPC, and relayer behavior.

## 11. Architecture-freeze proof disposition

The original eight open obligations are now closed at design level. They are
kept here as a traceable disposition rather than deleted, so implementation
cannot silently reopen one with a convenient local mechanism.

| # | Obligation | Disposition and controlling section |
|---:|---|---|
| 1 | read-only size eviction with CPFP/status equivalence | closed by the sparse virtual sequence and bounded physical union in 12.1-12.2 |
| 2 | unified Missing/Conflict waiting without lost wake or invalidation gaps | closed by atomic park, ordered epoch slicing, final-availability coalescing and resolved-key demotion in 13 and 19 |
| 3 | one token across promotion/replacement/leases/deadlines | closed by global non-reused entry version plus expected state/rank/deadline in 14; current attribution is read at completion |
| 4 | trusted progress and bounded Remote Sybil cost without victim undo | closed by asymmetric Remote/Proposal/ConflictHistory/ChainRecovery/Work partitions, per-peer/global limits and fair admitted-owner heads in 7.1 and 16; Remote cannot borrow trusted reserve, while sustained remote admission under unlimited connection Sybils is explicitly out of scope |
| 5 | total commit and exact bounded effects | closed by one immutable removal-union plan in 12.5 and static ordinary/critical journal regions in 15 |
| 6 | reorg ownership, persistence and assembler authority | closed by per-entry `RecoveryRetained`, bounded generation swap, v2 explicit-save snapshot and reset/full generation sequencing in 18 |
| 7 | static critical-path comparison | closed by the counted nominal paths in 22.1; benchmark remains an explicitly deferred falsification gate |
| 8 | material deletion target | closed by the module envelope in 22.2: at most 14k production Rust lines, tests/benchmark reported separately |

“Closed at design level” is not a claim that code already implements the
proof. Implementation starts only after the global contradiction audit in
section 23 passes. Every phase must preserve these dispositions in code,
reference-model tests and the review behavior ledger; otherwise that phase is
reverted instead of adding compensating state.

## 12. Proof note: accepted-pool displacement and immutable capacity planning

The first open obligation cannot be closed by wrapping the current
`limit_size` loop in a generic transaction. Its eviction key contains mutable
descendant aggregates. Removing one closure changes the keys of surviving
ancestors, so reproducing the current step-by-step result with an ordinary
snapshot iterator would silently change policy; reproducing a mutable
`PoolMap` in full would recreate the rollback system under another name.

More importantly, there are several distinct reasons an accepted entry can be
removed. They must be explicit policy proofs inside one plan, rather than
being treated as interchangeable victims:

| Cause | Authority to displace | Required proof |
|---|---|---|
| chain commit/reorg/expiry | authoritative chain or configured expiry policy | current snapshot/event and complete dependent closure |
| input RBF | incoming transaction | complete bounded conflict closure plus every RBF rule and final fee floor |
| pool size/residency | configured capacity policy | candidate participates in the same virtual package-eviction sequence; if it loses, the whole admission is rejected without removing unrelated entries |

This classification is also an attack boundary. Current cell-reference escape
performs physical displacement without the RBF incremental-fee proof: a spender
can evict arbitrary dep readers and their descendants even though it does not
conflict with them in the mempool ordering model. Section 12.3 rejects that
policy instead of legitimizing it with another cap. A future code path may not
invent another removal cause without adding a fee/chain authority proof.

### 12.1 Exact sparse virtual eviction policy

The planner preserves the existing stepwise CPFP/status policy without
performing its mutations:

1. compute the bounded mandatory RBF union;
2. validate the candidate and build the logical post-union pool view;
3. select the current virtual minimum `Pending`, then `Gap`, then `Proposed`
   root, expanding and deduplicating its descendant closure;
4. subtract that closure from surviving virtual ancestor aggregates, re-rank
   only the affected sparse frontier, and repeat until both size budgets fit;
5. reject without mutation if the candidate is selected, no candidate exists,
   or the complete physical union exceeds the configured displacement cap;
6. otherwise apply exactly the already validated union and insertion.

This reproduces the current observable eviction decision from one immutable
starting state: a parent loses CPFP protection when its paying descendant is
virtually selected, and status priority/self-eviction remain unchanged. It
avoids an unnecessary mempool-policy change and the stale-package protection
bias that a once-frozen key would create.

The plan can be constructed without a population scan. Entries affected by
mandatory removal are surviving ancestors of the bounded removal cohort, at
most `cohort_cap * max_ancestors_count`; insertion additionally changes the
candidate's bounded causal ancestors. A sparse overlay holds both frontiers,
their adjusted aggregates and the candidate. Each virtual removal updates only
surviving ancestors of the bounded removal set. Selection merges the unchanged base
eviction index with a small ordered overlay, ignoring stale base keys for
overlay members. Every newly selected closure is traversed under the same
total cohort cap. Thus ordinary work is bounded by transaction edges plus the
configured local cohort and its bounded ancestor frontier.

This proof depends on a stronger accepted-pool construction: an ordinary
admission cannot introduce an accepted causal parent after already accepted
required children. Waiting children move only after their parent is accepted,
while a reorg recovery batch is planned in topological order. Consequently a
normal candidate has no pre-existing required descendants; otherwise its
affected frontier could be pool-sized. Conditional cell-reference ordering is
not present in this graph and cannot enlarge the frontier.

### 12.2 Why the physical cap remains

Without a complete-union cap, one remote admission can force removal and
effect publication proportional to the pool population while holding the pool
write lock. With a cap, a very large high-fee transaction may be rejected when
fitting it would require too many independent tiny removals. This is a real
policy trade-off, not an implementation accident. The final design must choose
and test one of:

- explicit bounded rejection (simplest and strongest per-request DoS bound),
- or a separately budgeted maintenance mechanism that creates a capacity
  reserve before retry, without partially applying the candidate admission.

The second option is accepted only if it does not introduce another payload
owner or an indefinite privileged queue. Until that proof exists, bounded
rejection is the safe default.

### 12.3 Separate causal dependency from conditional block ordering

The `develop` graph and the current branch conflate two relations:

```text
causal producer -> consumer/dep-user
    The latter cannot resolve or be mined without the former.

conditional dep-reader -> spender
    This order is required only when both transactions are selected for the
    same block. The spender is valid without the reader.
```

This is not merely a naming problem. Today `get_tx_parents` inserts both into
`TxLinks`; consequently a popular dep cell inflates ancestor counts and CPFP
weights, removing/expiring one reader cascade-removes an otherwise valid
spender, and admitting the spender may evict arbitrary readers without an RBF
fee proof. The `develop` integration policy demonstrated the amplification by
accepting one spender only after evicting 1,002 of 2,000 dep readers. The
current bounded patch changes that to rejection, but retains the mistaken
model and creates a cheap pinning surface.

Current pool-overlay resolution exposes the order dependence: a dep reader
before the spender is valid, while resolving the reader after an accepted
spender reports `OutPointError::Dead`. Block validation, however, accepts the
reader-before-spender order and accepts the spender alone. Therefore the target
role-aware resolver and `PoolMap` causal graph contain only
accepted producers of inputs, direct cell deps, dep-group cells and expanded
dep-group members. Only causal edges participate in:

- ancestor limits and CPFP score;
- descendant invalidation/removal;
- RBF and size-eviction closures;
- proposal-package requirements.

The outpoint index still records every dep reader. When an attached block
spends an outpoint, those readers and their **causal** descendants are removed
as dead. Merely accepting or evicting a pool spender does not remove readers,
and removing a reader never removes the spender.

Resolution must implement the same separation, or admission order recreates
the pinning bug. Input lookup observes accepted-pool spends (or enters the
explicit RBF path). Cell-dep/dep-group lookup reads the chain or accepted
producer's pre-spend output/data and ignores an accepted **consumer** of that
outpoint; it still records the causal producer dependency. Consequently reader
then spender and spender then reader converge to the same accepted set. A
spender admitted first cannot cheaply suppress every later user of a popular
dep. Final revalidation uses the same role-aware provider.

Block selection remains consensus-safe without a persistent second graph:

1. select proposed causal packages using the existing CPFP policy;
2. over only the selected entries, index selected inputs and selected expanded
   dep outpoints;
3. add `reader -> spender` for each selected matching outpoint, ignoring a
   same-transaction input/dep self-edge;
4. add the selected causal edges and perform one deterministic topological
   sort before returning the template transactions.

There is at most one selected spender per outpoint and each conditional edge
corresponds to one selected dep occurrence, so construction and sorting are
`O(selected txs + selected causal edges + selected expanded dep occurrences)`,
not `O(pool)` or `O(readers * spenders)`. Accepted-state charging already
counts expanded dep occurrences; selector transient memory gets its own block
work budget.

Conditional edges can legitimately cycle—for example A reads x and spends y
while B reads y and spends x. Each transaction alone is valid although no block
can contain both. Such a cycle is not accepted-state corruption and never enters
the causal graph. The selected-set sorter deterministically drops the weakest
cycle member plus its selected causal descendants and re-sorts the bounded set.
RBF's permissive input resolve is followed by the same final role-aware
dependency check. Recovery/persistence replay is parent-first and revalidates
each transaction.

This intentionally supersedes the non-consensus `develop` escape policy. The
observable replacement is: in either arrival order, all 2,000 readers and the spender may remain
pending; a template containing both orders readers first; a template may mine
the spender alone, after which normal chain reconciliation rejects the now-dead
readers. Required regressions cover both admission orders, high-fee
spender-first selection, popular dep fanout, reader expiry/removal,
same-input-and-dep, RBF final liveness,
selected-set cycle containment, proposal-window independence and normal mining.

### 12.4 PoolMap construction consequence

`PoolMap` should expose `plan_mutation` and a total `apply_mutation`, not an
`add_entry` that discovers policy failures while deleting entries. Primary
accepted entries are the recovery source; links, outpoint/status/sort indexes,
aggregates, and totals are rebuildable projections. The planner checks all
arithmetic, touched memberships, container reservations, and exact full-hash
identity under the write guard. Once apply begins, no transaction-shaped
error remains. Derived drift causes a rebuild and re-plan before mutation,
never rollback or service fail-stop.

### 12.5 RBF atomicity without nested undo

`develop::submit_entry` performs this sequence under the pool write lock:

1. check RBF and, after a possible tip change, revalidate the candidate;
2. physically remove conflict victims and descendants;
3. publish victim conflict history and reject callbacks;
4. call fallible `add_entry`;
5. run mutable `limit_size`, which can evict the new candidate itself;
6. asynchronously try to recover newly unblocked history, ignoring queue-full.

Steps 3-6 explain the historical lost-victim, false-callback, failed-recovery
and restore-before-recover defects. The current branch adds a pool journal,
cell-ref local restoration, coordinator entry/cohort snapshots and commit
abort/restore to compensate. Those transactions are not an RBF requirement;
they are consequences of discovering decisions after the first write.

The target has one top-level immutable `PoolMutationPlan` built while the
`TxPool` write guard makes its base stable. Subplanners are pure and may return
constraints or IDs, never mutate:

```text
PoolMutationPlan {
    base_tip/generation,
    candidate_full_hash/status/prepared_entry,
    removal_union: [(full_hash, expected_status, cause)],
    causal_projection_delta and post-state totals,
    retained_conflict_subset plus terminal overflow,
    exact kernel delta and immutable effect batch,
}
```

Planning proves, in order, full-hash identity; current snapshot and proposal
status; complete bounded direct-conflict/causal-descendant closure; every RBF
rule and size-based incremental fee; candidate inputs/deps/headers in the
virtual post-removal pool; causal ancestor bound; sparse virtual size/resident
eviction including self-eviction; touched projection equations and arithmetic;
ConflictHistory quota; and exact ordinary journal capacity. All removal causes are
deduplicated before their effects are constructed.

The final `TxPool -> kernel` section reloads the Ready owner and checks
`(hash, version, expected state, rank)` plus the pool base generation. A stale
fact returns `Stale` and replans. Apply then moves only prevalidated values:
remove the union, apply prepared projection deltas, insert the candidate,
consume/move kernel owners and append the effect batch. Both authority locks
remain held until the complete pair is visible. Storage required by touched
maps/vectors/journal is reserved during Plan; allocator abort is process-level,
not an ordinary rejection path.

There is consequently no subject closure that can open another transaction:
RBF, capacity and dependency helpers only add facts to the same plan. A fault
in Plan leaves both authorities byte-for-byte unchanged. A defect panic during
Apply takes the bounded generation-reset/quarantine path from section 20; it is
not treated as a recoverable user error and cannot justify reintroducing generic
undo. Differential tests inject a non-Apply outcome after every planning step
and compare the full reference state, while successful Apply is compared to a
from-scratch rebuild.

## 13. Proof note: one Wait owner without lost wakeups

`WaitingParents` and `ConflictCache` differ in policy, but not in ownership or
execution semantics. Both own a raw transaction that cannot currently resolve,
both are keyed by unavailable cells/headers, and both must run the same
authoritative resolver after availability changes. Retaining two stores is
therefore accidental architecture.

The unified key is exact and typed:

```text
DependencyKey = Cell(OutPoint) | Header(Byte32)
```

`Wait.reason` records policy/telemetry (`Missing` or `Conflict`) but does not
select another execution path. Relay parent requests are derived from cell
keys; they are effects, not scheduler ownership. Successful resolution records
all canonical raw inputs/deps and expanded dep-group members needed for later
accepted-pool dependency handling. A fail-fast resolver may park on a subset
of currently unknown keys, but it must park atomically on at least one actual
unknown key and re-resolve from current state on every wake; collecting all
discoverable keys is preferred to reduce retries.

### 13.1 Atomic park protocol

A worker first resolves under a `TxPool` read snapshot. To complete with a
miss, it reacquires `TxPool` read and then the kernel, in the universal lock
order. It rechecks the reported keys while the pool cannot change:

- if any selected wake condition is already satisfied, the entry is requeued;
- otherwise the leased entry becomes Wait and all canonical reverse edges are
  installed before the pool read guard opens.

Every accepted-pool or chain transition that can change availability holds
`TxPool` write and then the kernel while recording the corresponding wake
event. Therefore availability cannot move between the final check and reverse
registration without either being observed or leaving a later level-triggered
wake. Worker notification is only a hint; queue membership remains the source
of truth.

### 13.2 Bounded fan-out wake

A popular cell may have a pool-sized waiter bucket. Waking it in one pool
mutation would violate the write-lock work bound. The reverse index therefore
uses a derived, payload-free wake task:

- each changed key receives a monotonically increasing availability epoch;
- each Wait edge stores the epoch observed when it was installed;
- each reverse bucket is an ordered set of `(full_hash, version)`, and a unique
  dirty-key task scans one bounded ordered range after a stable last-key cursor,
  waking only edges older than its target epoch;
- a concurrent later change records a pending newer epoch; it never resets the
  active cursor, and starts one follow-up pass after the current pass finishes;
- re-waiting at the current epoch is not immediately awakened again.

A mutable `HashSet` iterator position is explicitly forbidden as the cursor:
insertion, removal or rehash can skip members after a slice boundary. Ordered
last-key resumption remains well-defined under mutation. An edge inserted at or
before the cursor has already observed the active epoch and must not be woken by
that old event; an edge for a later epoch is covered by the pending follow-up
pass from the beginning.

This is the generalized form of the conflict-cache cursor/rerun requirement,
but it is attached to the one Wait reverse index and owns no transaction. Fair
round-robin draining across dirty keys prevents a churned hot key from starving
other parents. The task and edge bytes are included in the kernel budget.

Pool acceptance/removal, attached/detached block outputs and spends, and
main-chain header changes produce typed availability events in the paired
`TxPool -> kernel` transaction. A woken entry always re-resolves: an outpoint
that became permanently dead terminates, one still consumed by another pool
entry waits again, and one now live proceeds. No event directly declares a
transaction executable.

Paired plans compare pre/post authoritative availability and coalesce a key to
one epoch advance. RBF remove+insert must not publish a transient "free" input
when the final candidate still consumes it. This same-mutation rule prevents a
hostile replacement stream from generating useless pool-sized wake scans.

### 13.3 Conflict history capacity

Transactions removed by successful RBF can be inserted directly as
`Wait(reason = Conflict)` in the same paired plan. A bounded ConflictHistory quota
replaces the separate conflict-cache count/byte budget. The pool plan chooses
the exact retained subset before mutation. Removed history is ordered
causal-parent-first, and retention is closure-safe: a child is kept only when
its removed causal parents were retained earlier or its requirements remain
available outside the removal union. Overflowed history is terminalized
deterministically, as the bounded cache already permits. Thus kernel capacity
cannot make the accepted-pool apply fail, retained quota is not wasted on a
permanently parentless suffix, and no transfer window exists.

This construction removes conflict-cache payload ownership, recovery tickets,
separate outpoint indexes, and transfer generations while preserving bounded
historical recovery. It also removes `RaceLost`: all verified conflicting
candidates remain Ready. The single commit driver plans the strongest current
candidate; a final rank/version check retries if a stronger candidate arrived,
and only a successful pool plan removes or archives direct staged losers. A
failed winner terminalizes without ever displacing/restoring its competitors.

## 14. Proof note: one entry version is sufficient

Raw transaction hash remains the lifecycle identity; verification-cache
identity remains the witness transaction hash. A kernel-global monotonic entry
version is assigned on admission/readmission and on every transition that can
make an old worker/commit completion applicable again. Leases and commit plans
carry `(full_hash, version, expected_state)`.

- checkout/requeue/retry and trusted witness replacement advance the version;
- removal followed by the same raw-hash admission receives a new global
  version, so it cannot ABA to an old lease;
- same-witness `Remote -> Proposal` promotion changes attribution and exact queue keys but
  not worker-validating payload, so it need not invalidate active work; a
  completion moves in only its computed payload and derives the next queue/rank
  from the entry's **current** source/deadline, never from lease-captured source;
- a Remote verification may initially use its declared-cycle permit. If it
  stops only because that source-specific limit was reached, completion first
  reloads current attribution; a concurrent Proposal promotion acquires
  trusted work credit and retries once at the consensus limit. Other script
  failures are source-independent. Thus promotion neither invalidates useful
  work nor lets a stale remote declaration reject a trusted submission;
- a commit plan captures the separately ordered Ready rank as well as version,
  so a source/rank promotion makes the plan stale even when worker work stays
  valid;
- a different trusted witness advances the version and restarts from Resolve
  (or a later stage only if reuse is independently proven); verify-cache reads
  use `wtx_hash`, never raw hash;
- an untrusted different witness cannot repeatedly replace an active owner and
  burn completed work. It receives typed retry/duplicate until the bounded
  lease reaches a terminal/retry point; a bad-witness quarantine is keyed by
  `wtx_hash`, so it does not poison a later variant of the same raw hash.
  Trusted Proposal replacement remains immediate. Local direct submission is
  a separately charged transient borrower and never rewrites retained source,
  witness or deadline; its final paired commit consumes/settles any same-hash
  owner. Lease/deadline bounds
  and per-peer work quotas cap first-witness pinning without adding a second
  witness owner;
- exact queues remove old keys on promotion, so they need no lazy-ticket token;
  every asynchronous deadline/maintenance item carries the same entry version
  and expected deadline/state. Matching only a timestamp is insufficient: a
  remove/re-admit can reuse that timestamp and let an old task expire the new
  owner.

The pair `(incarnation, revision)` therefore encodes two kinds of ABA that one
non-reused global value already distinguishes. Two tokens remain justified
only if the second report or a concrete transition trace demonstrates a stale
operation that passes the one-version plus expected-state/rank checks.
`EntryVersion` is a checked process-global `u128` owned by the stable kernel
shell and is never reset by an entry-generation swap. Exhaustion closes Remote,
quiesces work and requests an orderly process restart; it never wraps or becomes
a transaction rejection. Given bounded transition throughput, reaching `u128`
exhaustion is physically outside node lifetime, while this rule keeps the proof
honest without adding a second generation token.

## 15. Proof note: total effects without service fail-stop

Stable post-state publication remains necessary, but the current maximum
reservation (approximately the accepted pool plus a block plus all pipeline
records) is a consequence of unbounded mutation cohorts, not a fundamental
requirement. Under the one physical displacement cap, an admission emits at
most `O(cohort_cap + 1)` terminal/accept records. Reorg and clear use bounded
plans or one coalesced reset effect.

The journal is owned by the stable kernel shell and preallocated, with globally sequenced records
and two authority regions: ordinary and chain-critical. The ordinary region
has an untrusted ceiling plus trusted headroom: Remote outcomes cannot consume
slots required by Local, Proposal or bounded maintenance, while trusted work
may use currently idle untrusted slots without evicting an admitted batch.
The critical region is a **latest-authority register**, not an ordinary FIFO:
it owns one preallocated maximum-size reset/full record and replaces an older
unsent chain authority with the newer `(sequence, chain_generation)` record.
Ordinary traffic can consume none of it. This overwrite is safe because the
record describes complete assembler authority rather than per-transaction
callbacks; it is necessary because an authoritative chain switch cannot return
Backpressure merely because its previous reset has not reached the assembler.
Its constant-size `GenerationReset` variant carries pool/kernel/relay epochs and
causes endpoints to discard or reconcile older derived work. It is the terminal
settlement for entries deliberately discarded by an over-bound or defect reset;
the design never attempts to place a population-sized hash/callback vector in
the critical slot.
The immutable effect batch (including bounded typed reject data and callback
snapshots) is built and exactly charged during read-only Plan. While
`TxPool -> kernel` is held, an ordinary Plan either proves that its matching
region can hold it or returns Backpressure without mutation. A chain Plan proves
that its coalesced record fits the statically sized latest-authority register
and then replaces the older value. Apply moves the prepared value before either
lock opens.

There is no dynamic permit/reservation object, reservation ID, or
credit-across-lock protocol. A command that observes full capacity releases all
locks and waits on a level-triggered capacity condition before replanning.
Sequence is append order at the state linearization point. Queued, active and
quarantined endpoint payloads remain charged until acknowledgement, bounded
timeout disposition, or deterministic drop.

An ingress request is already a transient payload borrower before kernel
admission. The bounded service/relay channel and concurrent handler set are
covered by an `Ingress` work limit. A Remote pre-admission path waits for one
terminal-result journal opportunity **before** taking authority locks; if it
cannot, the bounded ingress request remains the charged owner and no relayer
filter is falsely settled. This short-lived opportunity is not a mutation
reservation carried across locks: admission still performs the exact ordinary
capacity predicate in its final Plan, and cancellation releases the ingress
borrower.

Queue wakeups are level-triggered. A publisher task panic does not discard the
journal; a supervisor restarts the publisher against the same queued/active
batch. Publication performs only bounded dispatch; it never calls arbitrary
code while holding a state lock.

External endpoints are not authoritative tx-pool state and may not fail-stop
it:

- callback panic is contained. A hung blocking callback consumes at most one
  charged quarantined endpoint slot; timeout opens the circuit so no further
  blocking task is launched and later callback records are observably dropped;
- relay backpressure has bounded retry and an explicit relayer timeout/reconcile
  path instead of pinning the FIFO forever;
- recent-reject persistence is observability and reports/drops a failed write;
- block-assembler updates remain a coalesced authoritative snapshot plus wake
  token, preserving the existing `update_full/reset` priority contract.

Critical storage alone is not sufficient: a hung ordinary endpoint must not
head-of-line block reorg/reset. The critical dispatcher may therefore pass an
older ordinary record, and a newer authority may replace an unsent older one.
Every assembler effect carries the global sequence and pool/chain generation;
the assembler ignores a later-arriving older partial update after a newer
critical reset/full authority. Ordinary callbacks retain
FIFO disposition within their endpoint, and a timeout/circuit-open outcome
advances that endpoint. This separates mutation ordering from fallible endpoint
execution without allowing an old optimistic update to overwrite new chain
authority.

The state guarantee is *journaled after committed state*, not impossible
exactly-once delivery to arbitrary fallible code. Publisher/endpoint failure is
observable and locally quarantined. If the untrusted ordinary ceiling remains
full, Remote admission/commit backpressures. Local/Proposal retain trusted
headroom, and bounded endpoint timeout/circuit disposition prevents one
trusted batch from holding it forever. Chain/admin transitions retain their
critical lane and the node service remains alive.

For a chain/admin command, detailed per-transaction callback/relay records are
used only when their ordinary batch already fits. Otherwise Plan chooses the
constant-size critical `GenerationReset` settlement before mutation. Chain
authority therefore never waits for attacker-filled ordinary effects, and the
observable loss of per-item diagnostics is explicit rather than a silent
post-commit failure.

This retains an effect journal as an independently justified mechanism, but
removes coordinator lifecycle effects, generic `EffectOutbox` dynamic
reservation state, population-sized submit formulas, and every
journal-triggered service fail-stop. Folding effects into an unrelated
deferred-task channel is rejected unless that channel can prove the same
static partition, ordering,
residency, restart, and critical-lane properties.

## 16. Proof note: resource partitions and budget-before-work

The ownership budget must be acquired before non-trivial remote work. Current
code performs non-contextual verification before coordinator admission and can
construct an expanded `ResolvedTransaction` in a worker before the result is
charged. Those windows invalidate a proof that only sums resident coordinator
entries.

Remote admission in the target performs only bounded wire/identity checks,
then takes `TxPool read -> kernel` to prove the full hash and unique short-ID
slot are absent while it charges the raw payload and installs
`ResolveQueued`. This nested read is necessary for the ownership XOR; a
pool-sized accepted-hash mirror is rejected as a second executable projection.
It then schedules work.
The Resolve worker performs non-contextual validation and contextual
resolution under fixed worker and scratch-memory credits. A budgeted cell
provider accounts every retained input, cell dep, dep-group member and loaded
data allocation as it is collected; exceeding the per-job/global scratch
credit terminates with typed resource backpressure before an uncharged result
can grow further. Verification likewise runs only after acquiring a fixed
worker/cycle/memory credit. Local direct execution remains synchronous by
design, but uses a trusted work lane so remote saturation cannot starve it.
“Local” is priority, not an assumption that RPC is inaccessible: the lane has
its own fixed concurrent count/bytes/cycles. Saturation returns a synchronous
typed busy/full result and never creates an RPC-pending owner.

Checkout acquires a bounded stage work permit before cloning any worker
payload. Its initial units cover that entry's known resident payload plus fixed
stage scratch. A budgeted provider extends the same permit before each
resolver allocation, only up to the per-job cap and global scratch ceiling;
failure terminates with typed resource backpressure rather than waiting while
growing uncharged state. The permit remains until the last worker-held
reference is dropped. The resting entry charge stays while the owner exists;
if expiry, clear, replacement or a generation swap removes it first, the work
permit continues to cover the borrowed `Arc` and stale completion only drops
it. The bounded overlap deliberately permits conservative double charging
while work is active. It is simpler and safer than adding a `RetiredWorker`
location, detached payload registry, cancellation acknowledgement or another
owner.

Pre-pool residency is partitioned rather than reclaimed by general eviction:

| Class | Purpose | Saturation policy |
|---|---|---|
| Remote | untrusted peer submissions | global and per-peer count/bytes; typed backpressure; no unrelated victim |
| Proposal | consensus-bounded trusted promotion/payload | dedicated reserve; deterministic replacement only inside this class if the independently justified proposal bound is exceeded |
| ConflictHistory | optional historical RBF victims represented as ordinary Wait owners | deterministic retained subset chosen in the pool plan; excess history terminalizes; cannot consume ChainRecovery |
| ChainRecovery | authoritative detached-reorg ownership | dedicated maximum retained batch; no Remote/Proposal/RBF borrowing; over-bound event uses generation swap |
| Ingress/work scratch | bounded service/relay requests plus active resolve/verify allocations and borrowed payloads | fixed channel/handler/workers and byte/cycle permits; held until the last request/worker `Arc` drops |
| Plan/admin/template scratch | bounded mutation plans, selected-block ordering and explicit-save snapshots | permits acquired before authority locks; ordinary work backpressures, chain has a critical plan reserve, orderly save quiesces workers or retries |
| Effect ordinary/critical | post-state publication | ordinary has Remote ceiling plus trusted headroom; neither can consume chain/admin critical slots |

Fixed partitions trade a small amount of peak utilization for a constructive
guarantee: aggregate remote occupancy cannot force a proposal/local/reorg admission to
run a multi-entry victim transaction. Borrowing is intentionally asymmetric:
Proposal may consume currently unused Remote capacity and thereby reduce new
Remote admission, but Remote never consumes Proposal, Recovery, critical-plan
or Local work reserve. No borrower is evicted; release is completion/expiry.
Idle trusted space is not silently loaned to remote traffic unless a future
constant-time revocation proof is added.
Configuration validation must guarantee that each trusted/work partition can
hold at least one maximum supported entry/job. Proposal reserve is derived
from the maximum supported ingress item/batch and configured fetch concurrency,
not an arbitrary magic count and not the entire consensus proposal window.

The configured memory equation is conservative and includes transient
borrowers rather than assuming owner removal frees their payload immediately.
Its named limits are configuration-derived parts of one envelope, not hidden
independent pools:

```text
accepted_limit
  + remote_global_limit
  + proposal_reserve + conflict_history_limit + chain_recovery_limit
  + work_limit + plan_admin_template_limit
  + ordinary_effect_limit + critical_authority_limit
  + disposal_limit + emergency_spare_limit
  <= configured tx-pool memory envelope

actual_accounted <= the corresponding limit for every term
```

Remote global and per-peer maximum equations are separately bounded. Proposal
borrowing is charged against unused Remote only, never against a trusted reserve.
Plan permits cover the full bounded removal union and sparse ancestor overlay
before `TxPool` is locked. Template permits cover selected causal/reference
edges. An explicit-save permit conservatively covers every borrowed accepted
or recovery payload until the atomic rename completes; if it is unavailable,
an ordinary save retries without holding state locks, while orderly shutdown
first quiesces ordinary workers. These permits may temporarily double-charge
an owner plus borrower by design, but no retained allocation is invisible.

Scheduler queues are exact per-owner ordered sets plus a set of runnable owner
heads. An owner at its active cap has no runnable head; completion reinserts
one. This gives fixed global concurrency, per-peer fairness, no lazy tickets or
population scan, and no second payload owner. Duplicate remote witnesses do
not gain another payload/budget slot; a Proposal different witness may replace
the payload by advancing the one entry version. Local uses its transient direct
borrower instead. All verification-cache lookups
remain keyed by `wtx_hash`.

## 17. Second external design report: verified and corrected

The `Plan/Apply Kernel (PAK)` report supplies the right mutation discipline,
but its proposed global topology is not yet safe or performance-feasible for
this codebase. The useful parts are absorbed as per-authority planning rules;
the recommendation to freeze most of the current coordinator is rejected.

| Report claim | Code/evidence check | Consequence |
|---|---|---|
| Decisions belong in read-only Plan and Apply is total | Correct and central | Adopt for `PoolMutationPlan` and kernel transitions; typed reject/backpressure are plan outcomes |
| Exact effects are part of the plan | Correct; v1.2 removes the remaining dynamic-credit premise | Build the immutable typed batch before mutation and check a preallocated ordinary/chain-critical journal region in the final paired Plan; do not carry a permit or reservation across lock acquisition |
| Readiness is a level-derived predicate | Correct | Exact runnable owner heads are state; notifications remain hints |
| One global State/actor makes accepted membership a phase | Logically possible but not shown performance-feasible | Reject as default: accepted-pool concurrency is an independently justified authority boundary |
| All 17 read sites already grab a snapshot and go | False | The audit's explicit regex finds 34 direct production `TxPool` read/write acquisitions in 11 files; `cloned_snapshot()` clones the chain `Snapshot`, not a coherent immutable `PoolMap` |
| Publish `Arc<StateSnapshot>` after every Apply with zero-lock reads | Missing the essential data-structure proof | Current PoolMap is mutable hash/B-tree/graph state. Full cloning is `O(pool)` per mutation; structural sharing requires new persistent indexes and allocation/RSS analysis, contradicting “zero new layers” |
| Pure workers need only immutable payload/lease | Incomplete | Resolution needs accepted-pool cells and chain snapshot. Supplying a global immutable pool snapshot has the cost above; reading current TxPool preserves parallel reads and needs final lease/CAS validation |
| One actor removes `recovery_lock` | False in v1.0; v1.2 supplies the missing construction | Adopt charged/persistable `RecoveryRetained` ownership in the pre-pool kernel. It removes the cross-await lock without making accepted membership part of a global actor |
| Effect capacity can be checked only inside global State | False; v1.2's static journal regions work with the two existing authorities | Under the universal `TxPool -> kernel` order, the final Plan checks exact batch bytes while both states are stable. A full region returns Backpressure before mutation; no dynamic credit or opposite lock edge exists |
| A wrong view cannot contaminate State because audit catches it | False | Ready, conflict and victim views choose the winner/removal plan. A wrong executable view changes primary state before a later audit; it must be validated/rebuilt at the touched boundary, not merely called derived |
| Defect-only fail-stop is unreachable from malicious input | Not a security proof | The valid duplicate-dependency integration input triggered a latent derived-index defect and Authoritative fail-stop. Bugs are reached through inputs; local rebuild/quarantine is required for availability superiority |
| Frozen `Committing`, `Invalidated`, dual tokens, relation counts, tickets, victim indexes and ConflictCache are necessary | Contradicts the report's own single-writer premise and G1 | Same-critical-section Plan/Apply has no commit interleaving; one global version closes ABA; ConflictCache remains a payload owner; executable relation/victim views still enlarge the proof surface |
| Worker panic needs no cleanup because lease expires | Partly useful | A lease deadline can make work eligible again, but expiry/requeue is still a state transition and active scratch credit must be released; it is not “no cleanup” |
| M1-M5 are mechanical, net-negative convergence | Understated | Wrapping TxPool, coordinator and effects in one actor plus immutable pool snapshots is a major rewrite; retaining old topology first would prolong the snowball rather than prove the smaller model |
| Delete only 1.3k-1.8k production lines | Does not meet the reviewability objective | It leaves most of the ~24.2k production tree and its executable projections; the target must estimate from a minimal module skeleton, not current-module retention |

The resulting synthesis is not a global PAK actor. It is a **Plan/Apply
discipline inside the two-authority Thin Transaction Kernel architecture**:

- accepted `TxPool` retains its concurrent read/write lock and read-optimized
  graph;
- the small pre-pool kernel is the only unaccepted payload owner;
- each authority has read-only Plan and total Apply;
- the sole paired transaction is accepted commit/reorg/admin, ordered
  `TxPool -> kernel`; exact effects use the kernel's static journal partition;
- no state lock is held while waiting for work, I/O, or capacity;
- touched executable projections are constructively updated and locally
  rebuildable before they can direct a mutation.

This keeps PAK's strongest idea while avoiding its unproved snapshot memory,
single-writer read-contention, global failure domain, and retention of the
current coordinator's accidental mechanisms.

### 17.1 v1.2 delta

The revised report corrects one major contradiction and contributes two useful
architectural improvements:

- deleting persistent `Committing` now follows from its same-critical-section
  commit premise;
- reorg replay becomes charged/persistable ownership data, which can replace
  the cross-await recovery lock and its partial-save/deadlock window;
- exact effects can use a kernel-owned static ordinary/critical journal
  partition, so a final Plan returns Backpressure before mutation instead of
  carrying dynamic credit across lock acquisition.

These are absorbed into sections 8, 15, 18 and 21. The report still does not
repair its global topology proof: current `cloned_snapshot()` is the chain
snapshot, not an immutable PoolMap; the calibrated audit finds 34 direct
production guard acquisitions before wrapper-level counting; an
`Arc<StateSnapshot>` per Apply is `O(pool)` unless a
new persistent graph/index system is introduced. It also still keeps dual
tokens, `Invalidated`, stronger-count relations, victim indexes and
payload-owning ConflictCache, and still treats an input-triggerable software
defect as service-fatal. Those recommendations remain rejected.

The static effect regions need one qualification absent from the report:
configuration must prove that each region can hold the largest indivisible
ordinary or authoritative batch. Otherwise a command larger than its region
does not experience temporary backpressure; it can never make progress.
Ordinary oversized batches are deterministically rejected or split before
Plan. An authoritative chain/admin batch is coalesced to the bounded reset
record and, if its state closure is also over-bound, uses the generation-swap
fallback in section 18. Waiting is allowed only when an already committed
batch can release enough space, and the capacity predicate is level-triggered.

Likewise, spelling `plan` without `Result` is useful API discipline, not a
proof that defects are unreachable. Arithmetic, allocation and projection
bugs can still be triggered by legal hostile input. The target therefore uses
the explicit `PlanOutcome::{Apply, Reject, Backpressure, Stale, Repair}` and
contains touched-view defects by rebuild/replan or generation reset instead of
turning them into a tx-pool service fail-stop.

Its reorg switch is described as `O(pool)` and “bounded by pool limits”; that
is a memory bound, not an acceptable per-command work bound. The target keeps
the section-18 generation-swap fallback for an over-bound authoritative
closure/effect set. `RecoveryRetained` is a location inside the existing
pre-pool authority, not evidence for a global actor or for persisting remote
unverified traffic.

## 18. Proof note: reorg, clear, persistence and assembler authority

Reorg is not one bounded synchronous mutation. The first phase switches the
pool snapshot/status/membership, installs retained ownership and journals an
immediate chain-critical blank-template reset for the target generation.
Detached transactions are then topologically rechecked one at a time without
the pool write guard; only after retained recovery completes may the
authoritative full-template refresh publish that generation as complete. The
immediate reset prevents mining on the old parent during recovery; the later
full rebuild exposes all recovered proposals/transactions at once.

The v1.2 report supplies a better construction than a lock held across this
awaitable interval: **the barrier is charged, persistable ownership data**.
Before the switch, detached transactions are deduplicated, parent-first sorted
and closure-safely truncated to the ChainRecovery quota outside state locks. The
paired `TxPool -> kernel` Plan pre-reserves entry/index capacity, rechecks exact
full-hash ownership and moves the bounded vector into ordinary
`RecoveryRetained` entry variants while changing the pool snapshot. This is
honestly `O(retained batch)` work under the lock, not a nominal O(1) container
swap that would require another batch owner/index. The configured bound or the
generation-swap fallback limits it; no transaction is held only in the reorg
handler. The same plan carries the exact critical reset record and a
consensus-bounded prepared candidate-uncle set, so the new chain snapshot and
the authority that invalidates the old template cannot be separated by
cancellation or journal saturation.

The same chain plan reclassifies the complete affected accepted set against
the target snapshot: all current `Gap`/`Proposed` entries, plus (when local
packaging is enabled) Pending pool entries named by the target proposal/gap
windows. Promotion preserves the existing mine-mode policy; demotion is
unconditional. Thus `Proposed`/`Gap` always has current-window justification,
while a non-mining node may conservatively leave an otherwise promotable entry
Pending. In particular, a detached proposal never leaves a `Gap` owner that
RPC flattens to pending while proposal packaging ignores it. Every
`Gap -> Pending` transition creates the ordinary level-triggered proposal hint;
template selection still reads authoritative PoolMap status. The work is
bounded by status/window indexes rather than a pool scan. If the affected set
exceeds the chain transition bound, the generation-swap fallback is used
rather than publishing a partial status set.

A trusted recovery drainer leases retained items in parent-first order and runs
the same direct validation/submit logic one transaction at a time, releasing
state locks across work. Raw payload remains in the kernel entry while leased,
so cancellation, clear, explicit save and query never create a location gap. A failed
parent produces ordinary typed terminal/dependency outcomes; it never asks a
transitive invalidation state to roll back. Cascading reorgs merge/deduplicate
the bounded incoming vector against the same kernel entry map and newly
attached transactions, then advance the target generation; only the latest
fully drained generation may refresh the assembler.

Persistence takes `TxPool` read then kernel, copies one charged immutable save
snapshot, releases both locks, and atomically writes a versioned v2 envelope:
causal-parent-first accepted transactions plus every `RecoveryRetained`/active
recovery-source raw payload with session/ordinal metadata. Restart requeues
those raw transactions for authoritative validation. Clear takes
`TxPool write -> kernel`, clears the retained generation and makes old drain
leases stale. Consequently `recovery_lock`, its exclusion protocol and its
credit-order deadlock are deleted rather than renamed.

This is snapshot completeness, not a write-ahead log. An orderly shutdown or
explicit save during replay is recoverable; an unexpected process crash before
the next atomic save can lose accepted and retained mempool work under the same
best-effort durability contract. Closing that window would require a per-reorg
WAL/fsync protocol and is rejected absent an independent durability
requirement, because it would regress the performance and failure surface of
ephemeral mempool state.

An authoritative chain event cannot be rejected merely because its pool
closure exceeds the remote cohort cap. If its affected accepted-pool closure
or effect batch exceeds the separate chain transition bound, the availability
safe fallback is a generation swap:

1. install the new chain snapshot with an empty fresh `PoolMap`;
2. swap/settle the old pre-pool generation and publish one coalesced pool-reset
   authority record instead of per-entry callbacks;
3. move destruction of the old maps outside the write lock;
4. retain the bounded eligible detached batch in the fresh kernel generation
   and drain it through the ordinary direct validation path.

The mempool is ephemeral, so losing optional residents is safer than blocking
chain convergence, applying a partial graph, or killing the service. The
fallback is observable and rate-limited; repeated swaps enter remote-admission
cooldown while chain/RPC remain alive.

Block assembler semantics are preserved, not redesigned: reset and
`update_full` remain mutually exclusive under the assembler template lock; the
authoritative full rebuild has the highest priority, while
proposal/transaction partial updates remain optimistic and may be skipped
because a later level-triggered/full update converges. Reorg appends the blank
reset at the chain-switch linearization point. When and only when the latest
retained generation drains, it appends the matching `update_full`; a newer
reset/full generation makes every older effect a no-op. Optional uncles are
filtered against the full rebuild's proposal IDs so proposals win; detached
candidate uncles cannot strand recovered transactions. A reset failure remains
charged and retryable, while the newer-generation rule prevents an old reset
from overwriting an already completed full rebuild.

Notification is only a latency hint. Every template read/build compares the
template's `(chain_generation, parent_hash)` with the authoritative TxPool
snapshot. A mismatch cannot return the old template: it applies the pending
blank/full authority under the template lock or returns retryable-unavailable.
Therefore a delayed/overwritten reset cannot mine the detached parent during a
long recovery, while a same-generation full rebuild still has highest priority.

## 19. Proof note: dependency invalidation without `Invalidated`

Wait wake edges alone are insufficient: Verify/Ready entries also retain a
resolved dependency snapshot that can become stale when an accepted producer
leaves. The kernel therefore has one additional rebuildable reverse projection
from canonical resolved `DependencyKey`s to Resolve/Verify/Ready owners.

Availability-loss events mark the relevant key epoch and schedule the same
bounded dirty-key drainer described for Wait. Each affected current version is
directly demoted to Resolve, drops its resolved/verified payload and releases
the corresponding charge. There is no persistent `Invalidated` location and
no recursive transition: fan-out is processed in fair bounded slices.
Resolver/verify completions and the commit driver compare dependency epochs;
the final pool plan always revalidates inputs, expanded deps, headers, status
and tip. Therefore an entry not yet reached by the background slice cannot
commit stale data. If its parent returns first, current resolution decides
whether work can be reused; wake history never declares validity.

A successful Resolve completion publishes these reverse memberships while it
still holds the `TxPool` read guard used for resolution and then takes the
kernel in the universal order. This closes the event-before-edge race without
a second pool acquisition: an accepted removal is either excluded by the read
guard and observes the installed edge later, or it happened before resolution
finished and the completion observes current liveness. A completion may not
drop the Pool read and later install resolved dependencies under a kernel-only
transition.

Accepted-pool removals caused by a candidate still remove the complete bounded
accepted descendant closure in that candidate's immutable pool plan. Only
pre-pool demotion is sliced. An over-bound authoritative chain removal uses the
generation-swap fallback in section 18.

## 20. Failure-domain and derived-view rule

No executable projection is allowed to masquerade as non-authoritative merely
because it can be recomputed. The safety rule is narrower and checkable:

- the entry map and accepted primary entries own payload/membership;
- a queue/index can nominate only `(full_hash, version)`;
- Plan reloads the owner and validates expected state plus every touched
  membership before the view can direct mutation;
- Apply holds the authority lock, uses pre-reserved storage, and has no ordinary
  error return;
- a missing/stale extra view key is ignored and repaired; a missing key can
  delay work but periodic/on-demand equation checks rebuild it;
- budget and ownership are computed from primary entries, never inferred from
  queue/index counts.

Preflight drift first attempts only a bounded touched-key reconstruction. A
full rebuild is never smuggled into an ordinary hot-path Plan. Because pre-pool
contents are unaccepted and ephemeral, a kernel defect requiring a population
rebuild goes directly through the prebuilt generation swap; it does not add a
snapshot/CAS rebuild protocol merely to preserve speculative work. Accepted
PoolMap primary entries are more valuable: the first full projection repair is
a cold, observable `O(accepted_limit)` rebuild under its write guard. This can
pause pool readers once but adds no normal-path lock, second snapshot owner or
CAS protocol. Repeated drift or allocation failure takes the empty
accepted-generation recovery path and advances the same gate.
None of these defects skips chain progress, persists uncertain state, or
cancels the whole tx-pool service.

The swap path does not allocate its escape route while already handling a
defect. Runtime construction keeps one prebuilt empty PoolMap/kernel generation
with a critical reset slot. After any use, its replacement is prepared outside
authority locks before Remote admission reopens. The authoritative command
supplies the chain snapshot to install (base snapshot for an ordinary command,
target snapshot for a chain command), so a partially written in-memory snapshot
is never trusted as the recovery source.

Swapping does not pretend that the old maps are already freed. Their complete
resident charge transfers to one sealed `DisposalPermit` returned by Apply.
That bundle has no lookup, scheduling, persistence or re-entry API; it is only
dropped outside the authority locks. Critical swap serialization permits at
most one such bundle per authority, and Remote admission remains closed until
destruction and emergency-spare replenishment finish. It is a transient memory
borrower, not a third transaction location, and prevents repeated hostile
resets from accumulating uncharged retired generations.

The independent reference model deliberately recomputes all views after every
generated command. Production uses cheap touched-boundary checks plus sampled
full equations/metrics; tests and debug builds run the full equation set. This
makes bugs observable without claiming the impossible security distinction
that a software defect cannot be triggered by malicious legal input.

## 21. Frozen logical model

### 21.1 Authorities and transient borrowers

There are exactly two retained executable authorities:

1. `TxPool`: accepted primary entries and chain-relative accepted graph.
2. `PrePoolKernel`: every retained asynchronous unaccepted payload.

Resolve/verify workers, the serial commit driver, exact scheduler indexes,
dirty-key wake tasks and effect publisher are borrowers or projections. They
never become a transaction location. A synchronous Local call owns its trusted
request on its stack under a work permit and either returns a definitive result
or commits; it is deliberately not RPC-pending. Detached recovery remains a
kernel owner while its direct drainer borrows the raw payload.

Every asynchronous borrower holds a bounded work permit from before its first
payload clone until its last reference is dropped. Entry charge covers resting
ownership; work permits cover borrower lifetime, including a stale borrower
whose owner was concurrently removed. The ownership equation below is about
membership and query visibility, while the memory equation also includes
those permits. A sealed generation awaiting destruction is covered by the
one-at-a-time `DisposalPermit`; it can neither execute nor become visible.

The global ownership equation is:

```text
accepted(full_hash) + prepool(full_hash) <= 1
```

and equals one for every retained/queryable transaction. The paired commit
holds both authorities, so its internal remove/add order is not observable.

### 21.2 Closed pre-pool states

```text
RecoveryRetained(raw, session, parent_first_ordinal)
ResolveQueued(raw)
ResolveLeased(raw, lease_version)
Wait(raw, canonical_keys, reason, observed_epochs)
VerifyQueued(raw, resolved)
VerifyLeased(raw, resolved, lease_version)
Ready(raw, verified, rank, resolved_keys)
```

The last six locations encode the earlier four semantic phases;
`RecoveryRetained` is a bounded trusted ingress location that replaces the
cross-await reorg lock and is included in every explicit save snapshot. There is no `Committing`,
`Invalidated`, `RaceLost`, second raw stage or transferable conflict location.
Raw is retained in every state so dependency invalidation can demote without
reconstructing ownership.

`rank` is exactly the section-7 `ReadyKey`; no module may introduce a second
candidate order. Worker scheduling may choose a different fair queue order
before Ready, but direct-conflict eligibility, the global Ready head and final
commit CAS all compare the same key.

### 21.3 Commands and total planning outcomes

Kernel commands are limited to admission/promotion, checkout, worker
completion, wait/wake slicing, expiry/peer removal, commit settlement and
administrative generation change. Pool commands are accepted insertion/RBF,
chain reconciliation, expiry/size maintenance and removal. Cross-authority
commands exist only for accepted commit/reorg/clear/remove.

Planning does not hide typed outcomes behind an error name:

```text
PlanOutcome<Delta> = Apply(Delta)
                   | Reject(PublicReject)
                   | Backpressure(RetryClass)
                   | Stale
                   | Repair(ProjectionKind)
```

`plan` is read-only and bounded. `Repair` rebuilds and retries once before
quarantine/cooldown/reset. `apply` takes a validated Delta, owns all required
capacity, makes no policy decision and has no ordinary error return. Effects
are immutable members of the plan and are journaled before authority locks
open.

### 21.4 Transition table

| From | Command/result | To | Notes |
|---|---|---|---|
| Nowhere | reorg switch retains detached tx | RecoveryRetained | bounded/charged/persistable per-entry parent-first metadata |
| RecoveryRetained | recovery drain lease | ResolveLeased(new version) | direct trusted validation; raw remains kernel-owned |
| Nowhere | admitted remote/proposal | ResolveQueued | charge and exact queue membership precede work |
| any pre-pool | same witness Remote -> Proposal | same semantic state | move attribution/queue rank; active worker remains valid |
| any pre-pool | Proposal different witness | ResolveQueued(new version) | release old derived claims and charge atomically |
| ResolveQueued | checkout | ResolveLeased(new version) | fixed global/owner scratch credit |
| ResolveLeased | valid resolved | VerifyQueued or VerifyLeased(new version) | publish all expanded dependency keys and exact charge; direct lease is allowed only when a Verify work permit was acquired before the lock |
| ResolveLeased | unknown/conflict | Wait(new version) | atomic current-pool recheck closes lost wake |
| ResolveLeased | reject/panic timeout | Nowhere or ResolveQueued | public reject, or bounded retry for infrastructure panic |
| Wait | newer dependency epoch slice | ResolveQueued(new version) | remove every old reverse edge once |
| VerifyQueued | checkout | VerifyLeased(new version) | capability/owner head is exact and level-derived |
| VerifyLeased | verified | Ready(new version) | one total rank plus direct input buckets |
| VerifyLeased | stale dependency/tip | ResolveQueued/Wait(new version) | no invalidation location |
| VerifyLeased | reject/panic timeout | Nowhere or VerifyQueued | cache key is `wtx_hash` |
| Ready | successful paired pool plan | Nowhere + Accepted | final version/rank/dependency and journal-capacity check |
| Ready | authoritative stale/missing | ResolveQueued/Wait | final pool revalidation, no speculative displacement |
| Ready | definitive policy reject | Nowhere | next Ready entry is naturally eligible |
| any pre-pool | remove/expire/ban/clear | Nowhere | one owner transition releases every charge/index |

Every stale worker completion is `Stale` and does not mutate. Every transition
has one inverse audit equation for its exact projections, not a generic undo.

### 21.5 Local containment of an impossible Apply defect

Preflight and reserved storage make ordinary Apply total. As a final
availability boundary, authority apply is panic-contained. If an unforeseen
implementation defect still unwinds after mutation starts, the locked
ephemeral PoolMap/kernel generation is exchanged for the prebuilt emergency
empty generation before either guard opens. The plan's authoritative
base/target chain snapshot is installed, the triggering full/witness hash is
quarantined, persistence for the bad generation is skipped, and the emergency
critical slot emits one reset authority record. Destruction and construction
of the next spare happen outside the locks.

`PrePoolKernel` is a stable shell containing the non-reused version clock,
effect journal/latest-authority register, current entry/index generation and the
single section-9 `DefectDomain`. Only the entry/index generation is exchanged;
already committed ordinary effects and global sequence/version clocks therefore
survive recovery. The spare, one-at-a-time disposal permit and Remote ingress
response belong to that DefectDomain. Its `DefectGate` first enters exponential
Cooling and repeated resets move it to Open rather than letting witness variants
force an endless reset loop. Chain reconciliation,
RPC, local direct work and bounded Proposal/ChainRecovery paths remain available;
an operator-visible half-open probe controls recovery. This does not claim an
unknown software defect is harmless—it constrains its blast radius below the
current whole-service cancellation and bad-state persistence boundary.

Current CKB release/prod profiles unwind panics. Preserving this availability
boundary is a build/CI requirement; changing tx-pool production to
`panic = "abort"` explicitly forfeits it. Allocator abort, process abort, FFI
abort and a panic while already unwinding remain process-level failures and are
outside any in-process atomicity proof.

This is intentionally a last-resort defect domain, not normal control flow and
not a rollback journal. Property/model/fault-injection tests must prove that
all known legal and hostile inputs exit through typed Plan outcomes instead.

### 21.6 Frozen resource equations

For a pre-pool entry `e`, `entry_charge(e)` is a closed function of its state:
retained payload allocation, state metadata, canonical key vectors and every
exact derived membership introduced by that state. It never relies on queue
length, an inferred location or a later audit. Accepted entries and PoolMap
projections remain under the configured accepted-pool count/bytes limits. The
complete runtime equation is the one in section 16 and is checked at each
transition boundary:

Here “exact” means a deterministic conservative upper bound from concrete
payload lengths and calibrated per-container/member overhead, not a claim to
observe allocator-private RSS byte for byte. The same charge function is used
for admission, transition, model audit and metrics; allocator/RSS benchmark
later falsifies whether its safety margin is adequate and affordable.

Immutable per-entry vectors use compact boxed/arc slices so charge follows
length. Mutable hash/vector containers are charged by allocated capacity, not
live length; deletion does not refund retained buckets. Growth capacity is
included in Plan before `reserve`, and deterministic shrink/rebuild occurs only
as bounded maintenance or generation replacement. This prevents churn from
turning “exact member count” into an unbounded allocator-capacity leak.

```text
accepted_authority_and_projection_charge <= accepted_resident_limit

prepool_entries_and_indexes
  + Remote + trusted/recovery reserves
  + work/plan/admin/template/effect permit limits
  + disposal permit + prebuilt emergency spare
  <= pipeline_resident_limit

total_tx_pool_envelope = accepted_resident_limit + pipeline_resident_limit
```

Defaults retain the existing two public envelopes (currently 1,000,000,000
accepted resident bytes and 384,000,000 pipeline resident bytes). Construction
derives every sublimit from the latter and rejects configurations where their
checked sum exceeds it. Configuration validation also proves:

```text
remote_per_peer_count/bytes <= remote_global_count/bytes
remote_attack_identity_floor = ceil(remote_global / remote_per_peer)
one maximum proposal ingress batch <= proposal_reserve
one bounded detached batch or one reset record <= chain critical reserve
one maximum legal plan/effect cohort <= its indivisible region
```

The implementation calculates conservative entry/index/permit overhead rather
than hard-coding wire bytes as resident cost. Metrics expose the configured
identity floor and saturation duration; they do not advertise a false honest
remote admission guarantee under an attacker that controls enough connections.

The limit of a live permit is charged, not the allocator's current observed
usage. This makes the bound independent of allocator timing and covers stale
worker/save/template `Arc`s after their primary owner disappears. A transition
computes old and new closed charges with checked arithmetic, validates the
class/global equations and reserves exact containers before Apply. Apply then
moves the already charged value and updates totals; it cannot discover a new
budget decision. Removal releases entry charge immediately but never releases
an independent borrower permit.

### 21.7 Frozen lock order and linearization points

`PrePoolKernel` uses one short-held synchronous mutex. It is never held across
`.await`, script execution, contextual resolution, filesystem/network I/O or
arbitrary callbacks. The only nested authority order is:

```text
optional serial/work/plan permit -> TxPool read|write -> PrePoolKernel
```

A kernel peek may occur before `TxPool` only if it is released first. No kernel
guard may acquire or await `TxPool`. Verification cache, endpoint, relayer and
candidate-uncle locks are ancillary and are never held while acquiring either
authority. The assembler's independent order is `template_lock -> TxPool
read`; state mutation only journals assembler effects and never acquires the
template lock. Candidate uncles are updated and released before taking the
template lock, so they create no reverse edge.

| Operation | Locks/permits | Linearization point |
|---|---|---|
| remote/proposal admission or promotion | resident/work policy, TxPool read, then kernel | accepted absence plus entry/version/index delta and any terminal effect linearize together |
| resolve/verify checkout | stage permit, then kernel | queued-to-leased version transition; permit already covers the returned borrower |
| successful Resolve completion / atomic park | work permit, retained TxPool read, then kernel | leased-to-Wait/Verify total transition and dependency edges install before Pool read opens |
| successful Verify completion | work permit, then kernel | leased-to-Ready total transition; dependency-loss events already target the leased owner and make stale completion harmless |
| atomic park after missing/conflict resolve | work permit, TxPool read, then kernel | Wait owner and all reverse edges installed before the final Pool read guard opens |
| accepted commit/RBF/size, including Local direct | commit/plan permit; optional kernel hint released; TxPool write, then kernel | final pool/kernel generation increment after both deltas and exact effect batch are installed, before either guard opens |
| reorg/clear/over-bound generation swap | critical plan permit, TxPool write, then kernel | target snapshot, complete retained/reset delta and replaceable latest-authority effect installed together |
| explicit save | admin snapshot permit, TxPool read, then kernel; no guards during I/O | in-memory v2 image captures one `(pool_generation, kernel_generation)`; filesystem durability occurs later at sync+atomic rename |
| effect publication | kernel checkout/ack transitions, no authority guard during endpoint work | state-visible effect linearizes at journal append; endpoint success/failure is a later charged disposition, not part of state commit |
| assembler reset/full | template lock, bounded TxPool read for full | generation-checked template swap; older partial/reset effects cannot overwrite a newer authority |

The commit-driver mutex serializes planning attempts but owns no transaction
and is never awaited while an authority guard is held. A separate save-writer
mutex serializes concurrent file replacements; it may span file I/O only after
the authority guards have been released, and no mutation path acquires it. A
save therefore represents its earlier state linearization point, while two
explicit saves cannot complete to disk in reverse order. Recovery drain
ordering is data (`session, parent_first_ordinal`), not a lock. These rules
delete every verified lock cycle involving `recovery_lock`, dynamic effect
credit and TxPool instead of choosing a more complicated order for them.

## 22. Static performance, maintenance and extensibility review

### 22.1 Critical paths

| Path | Target lock/work shape | Comparison |
|---|---|---|
| remote admission | one `TxPool read -> kernel` pair; exact full-hash/short-slot/quota/owner-queue update | ownership XOR is linearized without a pool-sized mirror; expensive noncontextual work moves behind bounded ownership |
| resolve | concurrent TxPool read during cell resolution; two short kernel transitions; fixed scratch permit | preserves develop/current parallel accepted reads; no global PoolMap snapshot clone |
| verify | kernel checkout/completion only around external parallel verifier | same worker parallelism, fewer lifecycle/ticket/conflict transitions |
| accepted commit | brief kernel peek; TxPool write; bounded pool plan; final kernel CAS plus exact static-journal capacity; total paired apply | no dynamic effect credit, generic undo, held losers, conflict relation reconciliation or post-mutation recovery |
| block template/RPC | existing TxPool read path; RPC takes TxPool then short kernel lookup only when needed | no actor hop and no `O(pool)` per-mutation snapshot publication |
| wake/invalidation fan-out | payload-free fair slices under kernel lock | prevents pool/chain write-lock fan-out and hot-key starvation |
| reorg | prebuilt retained vector moved through pre-reserved per-entry deltas in one bounded apply; bounded pool phase or generation swap; per-tx trusted direct drain | no second batch owner or lock across await; every explicit save sees complete accepted + retained ownership |

Normal adversarial complexity is bounded by transaction edges, configured
ancestor frontier and one physical cohort. There is no population scan on
admission/checkout/commit. Cold rebuilds and persistence remain explicit
operational paths with metrics. The target uses the same short synchronous
kernel mutex pattern as the current runtime but materially less work inside it.
Benchmark remains a later A/B falsification gate, not permission to retain an
unbounded or over-complex path.

For the nominal cache-miss remote-success path, counting only authority/store
critical-section acquisitions (not notification, metrics or verification-cache
locks) gives a reviewable static budget:

| Architecture | Nominal acquisitions | Evidence/interpretation |
|---|---:|---|
| `develop` | 7 | orphan read, verify-queue duplicate read and admission write, worker empty read and pop write, TxPool resolve read, TxPool submit write |
| current checkpoint | at least 16 | duplicate Pool+coordinator reads, admission Pool+coordinator, raw checkout/source/completion, Pool resolve read, verify checkout/source/snapshot/source/completion, Pool write plus coordinator begin/finalize; effect-credit locks are excluded |
| target conservative path | 10 | admission `TxPool read -> kernel`, five later short kernel entries, one concurrent TxPool resolve read, one Ready peek, and final `TxPool write -> kernel` pair |

The architecture freezes only the conservative path. Direct Resolve-to-Verify
leasing and a non-owning `(hash, version, rank)` Ready hint are legal derived
optimizations, but are not part of the correctness or static-superiority proof
and are not implemented during cutover. They may be added only after the final
A/B gate identifies authority-acquisition cost as a material regression; the
fallback path and final CAS remain authoritative. This avoids maintaining two
scheduler protocols merely to make an unmeasured nominal count equal to
`develop`.

For topology calibration, a reproducible source search on the frozen checkpoint
finds exactly 34 direct production `TxPool.read/write().await` acquisitions in
11 files under the explicit regex used by the audit. Wrapper consumers and
logical operations can yield a different count, so the design does not repeat
the external reports' unqualified 17/40/52 figures. The count supports the
two-authority boundary; it is not itself a performance measurement.

### 22.2 Maintenance surface

The intended production module skeleton and hard line envelope are:

| Production area | Maximum Rust lines | Scope |
|---|---:|---|
| accepted pool/planner/selector | 3,200 | primary entries, causal projections, sparse mutation planner, total apply, selected-set ordering |
| pre-pool kernel | 3,000 | closed entry states, exact indexes, quotas, scheduler and transitions |
| process/workers/reorg/admin | 3,000 | adapters, direct Local path, paired commit, bounded chain/admin flows |
| service/effects/persistence/callback | 2,200 | static journal, endpoint isolation, v2 explicit save and public service boundary |
| block assembler | 1,500 | existing authority protocol and proposal-wins uncle filtering, excluding tests |
| shared types/config/error/utilities | 1,100 | concrete non-generic protocol types and configuration validation |
| **total** | **14,000** | production only; tests and benchmark are counted and reported separately |

The following production mechanisms have a deletion requirement, not a rename
requirement: generic coordinator generics/bundles, capacity victim transactions,
undo/cohort snapshots, persistent Committing/Invalidated/RaceLost, relation
degree/stronger-count, lazy generation ticket heaps, second raw queue/stage,
payload-owning ConflictCache, generic EffectOutbox reservation map and
authoritative service fail-stop.

The target and escalation envelope is at most 14k production Rust lines
excluding tests and benchmark: at least ~10.2k lines leave the current ~24.2k
tree, which removes more than half of its ~16.9k growth over `develop`. Test
lines are reported separately. Exceeding 14k is not silently waived or repaired
with denser code: it stops implementation and requires a fresh architecture
audit proving which independently necessary mechanism cannot fit the reviewed
module envelope. A phase
that adds more production code than it removes must identify a still-open proof
obligation; otherwise it reverts rather than being patched forward.
The gate counts dedicated production `.rs` files after inline test bodies are
moved out; benchmark and test modules are separate counters, so adding a
regression cannot disguise production growth or deletion.

### 22.3 Extension rules

- A new submission source adds quota/rank policy, never a store or lifecycle.
- A new dependency condition adds a typed key/reason and resolver rule, never a
  second waiting owner.
- A new worker capability adds an exact runnable-head projection and work
  permit, never payload queue ownership.
- A new accepted displacement cause must state its authority, bounded closure,
  fee/precedence rule and effect set in `PoolMutationPlan`.
- A new side effect adds a bounded typed journal variant and endpoint failure
  policy, never I/O under state locks.
- An optimization may add only a rebuildable projection with an equation,
  charge and proof that a stale key cannot mutate the wrong owner.

This makes future review additive against a small set of rules instead of
requiring reviewers to reconstruct hidden cross-module ownership.

### 22.4 Deliberate compatibility and policy register

The redesign is not allowed to hide a mempool-policy change inside a safety
mechanism. Consensus validation is unchanged; the following observable
non-consensus differences are frozen and require explicit review evidence:

| Surface | `develop` / checkpoint behavior | Target behavior | Reason and compatibility evidence |
|---|---|---|---|
| unaccepted Remote saturation | fixed queue rejection in develop; checkpoint may displace weaker coordinator owners | global/per-peer typed backpressure; no unrelated pre-pool eviction | fee is unknown before resolve; sustained Sybil saturation is an explicit residual; relay retry and trusted isolation tests |
| staged verified conflict order | checkpoint source/size-fee rank plus Committing freeze | exact section-7 ReadyKey, no Committing bit | proposal priority retained; final RBF policy remains authoritative |
| failed RBF | victims may be removed, callbacked and restored | every rejection is mutation-free | removes false callbacks/loss; same public rejection |
| cell-ref reader/spender | persistent ancestry and escape eviction/rejection | accepted graph excludes conditional edge; selected template orders reader before spender | consensus-safe ordering proof and mining integrations in 12.3 |
| conflict-history overflow | bounded cache may forget victims | closure-safe deterministic retained prefix; excess terminalizes | preserves best-effort nature while eliminating transfer gaps |
| reorg over-bound closure | potentially unbounded work/fail-stop | observable ephemeral mempool generation reset with bounded detached recovery | chain convergence has priority; residual availability risk is recorded |
| Gap/proposal window | stale internal Gap can look RPC-pending and strand | exact demotion/promotion plus level-triggered proposal hint | restores normal template liveness without RPC-status change |
| explicit save | accepted raw v1 snapshot; replay interval excluded | versioned v2 accepted + recovery-owned raw snapshot | improves orderly recovery; unexpected-crash durability remains best effort |
| internal Apply defect | checkpoint can stop tx-pool service and disable persistence | one DefectDomain resets ephemeral authority and gates only Remote | bounds input-triggerable service DoS; process abort remains out of scope |
| verification identity | historical raw-hash lookup exists on detached replay | typed `wtx_hash` only | prevents same-raw/different-witness proof aliasing |

Every row has a behavior ID and both unit/reference-model and integration or
boundary evidence before production acceptance. Any further policy difference
is a design change, not an implementation detail.

## 23. Architecture audit gate (executed)

The global PAK actor is **no-go** until someone supplies a persistent PoolMap
snapshot design that beats the existing concurrent read lock in mutation cost,
RSS and review surface. Retaining the current coordinator with Plan/Apply
wrappers is **no-go** because it preserves mechanisms disproven as necessary.
Patching develop's independent stores is **no-go** because the ownership
registry required to close their races is the kernel again.

The Thin Transaction Kernel with per-authority Plan/Apply passed the restarted
audit as **GO with recorded non-blocking risks**. The signed evidence and the
concentrated blocker corrections are in
[`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md). The following remain hard
implementation gates:

1. no third retained payload owner and no persistent committing/invalidation
   state;
2. no generic undo or fallible operation after an apply linearization point;
3. no legal/hostile input path to service fail-stop;
4. fixed worker/scratch/residency/cohort bounds acquired before work and exact
   static-journal capacity proved before Apply;
5. concurrent accepted-pool reads and current block-template priority remain;
6. all current ledger behaviors either map to the smaller invariant set or are
   explicitly documented as superseded non-consensus policy;
7. production code converges toward the size gate before any optimization;
8. benchmark is deferred until explicit instruction, then must show no
   regression against the saved checkpoint/develop baselines.

The executed audit was read-only with respect to production code and produced
one signed-off matrix. It:

1. replay every verified `develop`/current historical defect and attack against
   the frozen mechanism that claims to close it;
2. enumerate the state/command product with the recomputing reference model and
   try to construct ownership, ABA, lost-wake, partial-Apply and stale-effect
   counterexamples;
3. derive the complete lock/wait graph, resource equations and trust-partition
   exhaustion cases, including worker/save/disposal borrowers;
4. check adversarial work bounds for fan-out, RBF union, virtual eviction,
   reorg/status classification, template ordering and failure recovery;
5. compare every deliberate policy difference with `develop` and identify any
   consensus/RPC/relay/mining compatibility change;
6. challenge the module/line envelope, extension rules and nominal/fallback
   critical paths for maintainability and performance feasibility;
7. classify each finding as design blocker, implementation acceptance test or
   explicitly recorded residual risk. A blocker changes the frozen document
   once and restarts this audit; it is never patched by adding an unproved
   state/index/undo mechanism.

The result is not a code-completion claim. Every implementation phase repeats
the global matrix; a new design blocker returns here instead of adding a local
state/index/undo patch.

## 24. Mechanism-by-mechanism necessity and superiority ledger

This table is the reviewer-facing proof index. “Necessary” means a concrete
counterexample survives without the mechanism. “Superior” compares the chosen
encoding with the smallest credible alternative, not merely with broken
`develop` behavior.

| Retained design | Necessity / concrete failure without it | Why the chosen encoding is superior | Required falsification evidence |
|---|---|---|---|
| two authorities (`TxPool`, `PrePoolKernel`) | develop queue/orphan/active stores lose ownership across pop, cancel, clear and re-admit; one global actor cannot cheaply snapshot current PoolMap reads | minimum owner count that preserves concurrent accepted reads while making all unaccepted locations one field | ownership command model; RPC/commit/clear race tests; no third payload search |
| closed seven-location encoding (RecoveryRetained plus four semantic phases) | a payload hidden in queue/cache/worker/reorg handler cannot be queried, charged, persisted or removed consistently | variants make payload/location combinations unrepresentable; retained reorg data replaces a lock and run status exists only where a lease does | generated transition partition; explicit-save restart and serialized location table review |
| one global entry version + expected state/rank | old worker can complete after requeue, witness replacement or remove/re-admit | one non-reused value closes both ABA families; avoids dual-token propagation and separate ticket generations | exhaustive stale completion traces, promotion and witness replacement tests |
| read-only Plan + total Apply | develop/current discover ancestor, size, handoff or effect failures after victim removal and need nested rollback | all policy/arithmetic/allocation/touched-index checks precede one bounded move; undo vocabulary disappears | fault point before every apply; state unchanged for every non-Apply outcome |
| single serial Ready commit driver | concurrent speculative commit requires Committing/RaceLost/restore and relation reconciliation | serialization exists today; final version/rank CAS handles new stronger work without persistent states | stronger-arrival race, failed-winner progress, local-direct race tests |
| global Ready order + per-input ordered buckets | verified conflicting candidates need deterministic strongest selection without unverified ownership | direct indexes answer only needed questions; removes transitive relation degree/stronger-count/tickets | model compare against brute-force rank/conflict selection |
| one Wait owner for Missing/Conflict | orphan and ConflictCache are the same unavailable-resolution lifecycle and transfers create gaps/stranding | reason is policy metadata; one raw payload, resolver and reverse key system removes a dormant third owner | multi-parent, expanded dep-group, conflict release/reblock, attached-output tests |
| resolved dependency reverse projection + final pool revalidation | Ready/active verification can retain a parent removed after resolution | background demotion bounds cleanup; final revalidation supplies immediate safety, so no persistent Invalidated cascade | removed expanded parent during verify/Ready; parent returns before slice |
| fixed asymmetric resident partitions | remote or RBF-history saturation otherwise forces unrelated victim transactions or blocks trusted/chain progress | Remote, Proposal, optional ConflictHistory and authoritative ChainRecovery remain one owner map but have non-interchangeable charges; constructive isolation deletes global prepool victim selection | saturate Remote/one peer/ConflictHistory, prove bounded admitted-peer fairness and proposal/local/chain-recovery progress, record remote admission residual |
| work scratch/cycle permits acquired before work | admission-time noncontextual verify and resolver expansion allocate/compute outside resident-state accounting | bounds peak, not just resting state; fixed workers alone cannot bound variable dep data | maximum dep-group/cell-data abort, worker panic/cancel credit recovery |
| primary PoolMap + rebuildable projections | accepted entries, links, weights, totals and indexes can drift and current assertions turn legal triggers into service failure | ownership source is explicit; touched preflight and cold rebuild prevent projection defects from becoming partial mutation | independent rebuild equality after every pool plan and injected drift |
| causal graph separated from conditional template order | `develop` counts dep readers as required ancestors, enabling fee-free mass eviction, pinning and false descendant invalidation | persistent graph contains only liveness dependencies; selected-set outpoint edges give consensus order with work proportional to block-selected references | popular dep fanout, high-fee spender-first, reader removal, chain spend, same input/dep and cycle-containment tests |
| exact sparse virtual displacement plan | mutable RBF/`limit_size` removal discovers later failure and changes CPFP keys while deciding | bounded overlay reproduces stepwise CPFP re-ranking and status policy, rejects over-bound work without mutation and supports total apply | differential full-policy tests; overlap/CPFP/status/self-evict/adversarial cap model |
| explicit displacement authority table | RBF, size and chain/expiry events delete entries under different authority and fee rules | reviewers can prove fee/chain authority and physical bounds per cause; conditional template order grants no displacement right | one regression/manifest row per cause and mixed-union cap tests |
| kernel-owned static-partition effect journal | callbacks/I/O under locks deadlock; unjournaled publication can be lost; unlimited or attacker-filled backlog retains payload and can suppress trusted work or chain authority | exact Plan check plus preallocated Remote ceiling, trusted headroom and replaceable latest-authority register makes append/chain replacement total without dynamic credit locks; endpoint isolation avoids service fail-stop | Remote/trusted saturation, consecutive critical replacement, publisher restart, callback hang/panic, relay timeout tests |
| charged/persistable RecoveryRetained data and retained generation head | save/clear/template can observe the pool after chain switch but before detached recovery; handler-local replay is absent from the save image | one kernel location removes the cross-await lock while preserving parent order, query ownership and explicit-save restart drain | save/clear/cascading-reorg/save-restart/template matrix |
| over-bound chain generation swap | authoritative chain event cannot be rejected and may invalidate pool-sized fan-out | mempool reset is bounded, recoverable and chain-safe; better than unbounded write lock or process death | hostile high-fanout block, persistence skip, assembler blank/full recovery |
| assembler authority journal and proposal-wins uncle filter | optimistic messages can be skipped; detached uncle proposal IDs can strand recovered Gap/Pending entries | preserves proven full/reset priority and makes hints level-triggered; orthogonal consensus liveness stays out of pipeline states | normal-template reorg tree, reset/full race, uncle conflict integrations |
| exact reorg status classification | an internally stranded Gap is RPC-pending but absent from both proposal and commit selectors | bounded status/window-index plan demotes outside-window entries before publication and emits a level-triggered proposal hint; no pipeline state is added | Gap/Proposed/Pending target-window matrix on mining/non-mining nodes and normal-template dependent-tree integration |
| one DefectDomain (prebuilt spare, DisposalPermit, Remote DefectGate) | a legal witness can trigger a latent projection/Apply defect, turning whole-service fail-stop into a reproducible DoS | one failure-domain abstraction swaps ephemeral authorities while guards remain held, keeps retired bytes charged and moves only Remote through Closed/Cooling/Open; no parallel recovery protocols | injected touched-view/apply panic, repeated distinct witness triggers, bounded retired generation, chain/RPC/local/proposal availability and persistence-skip tests |
| full hash ownership and `wtx_hash` verify cache | short-ID collision aliases owners; raw hash aliases different witnesses in verification cache | identities follow their actual security domains without duplicating lifecycle owners | collision and different-witness cache/replacement tests |
| reference model + test/review manifest | distributed state bugs escape example-only tests and reviewers cannot reconstruct intended behavior | small recomputing oracle makes every optimized view falsifiable; manifest ties history, tests and review questions | generated command differential; CI rejects missing/stale anchors |

No current mechanism is retained merely because it has tests. A test for a
deleted encoding is rewritten against the behavior/invariant it protected; if
no behavior remains, the test and its ledger row document the superseded policy.

## 25. Deletion-first implementation and review plan

Every phase ends in a checkpoint and a global architecture review against
sections 21-24. A failed hard gate reverts the phase; it is not repaired by
adding another state/index/undo layer.

### P0 — Formal design and review contract

- Promote this record to the permanent architecture design.
- Link it bidirectionally with `REVIEW_GUIDE.md`, `pipeline.md`, the security
  ledger/manifest and test inventory.
- Freeze state/command/transition/resource/lock/effect/displacement tables and
  mark unchanged versus deliberately changed mempool policies.
- Add the recomputing reference-model command vocabulary in test-only code.

Exit: documentation validators pass; every retained mechanism has one
counterexample and one falsification row; no production behavior changes.

### P1 — Minimal kernel model and vertical cutover

- Implement concrete, non-generic `PrePoolKernel` states, one version clock,
  exact owner queues, Ready indexes, Wait/dependency epochs and quotas.
- Move noncontextual remote work behind charged admission and work permits.
- Rewire resolve/verify workers and the serial commit driver to ID/version
  leases; keep local submission synchronous and retain direct per-transaction
  validation for the later recovery drainer.
- Integrate conflict history into Wait and delete `ConflictCache` ownership.
- In the same cutover delete coordinator/runtime generics, dual tokens,
  Committing/Invalidated/RaceLost, relation counts, lazy ticket heaps, capacity
  victim transactions and coordinator undo. Do not leave adapters that emulate
  the deleted state machine.

Exit: production lines are net-negative for the phase; ownership/model/fairness
nextest gates and targeted integration pass; global review finds exactly two
retained owners.

### P2 — Accepted PoolMutationPlan cutover

- Split PoolMap into primary entries and rebuildable projections.
- Implement read-only combined RBF/capacity planning with explicit
  displacement authority and one union cap.
- Restrict accepted links/weights/cascades to causal producers; add bounded
  selected-set cell-ref ordering and defensive cycle containment to
  `TxSelector`.
- Add exact sparse-overlay eviction and total apply with pre-reserved
  touched containers.
- Pair final kernel CAS/delta with pool apply; delete `PoolCommitJournal`,
  restore-before-recover, nested undo and fallible post-mutation handoff.

Exit: every failed candidate is mutation-free; successful plan/model states
match; CPFP/status/RBF/ancestor behavior matrix passes; no production `undo` or
ordinary fallible Apply remains.

### P3 — Dependency and chain failure-domain convergence

- Finish sliced Wait wake and resolved-dependency demotion.
- Move detached replay into bounded/charged/persistable `RecoveryRetained` plus
  recovery-source entries; delete `recovery_lock` and handler-local ownership.
- Preserve the two-phase retained generation head and exact assembler priority;
  remove pipeline-specific reorg cascades.
- Add over-bound authoritative generation swap and the single DefectDomain;
  remove authoritative service fail-stop and parallel quarantine/cooldown/
  circuit protocols.

Exit: legal/hostile integration inputs cannot cancel tx-pool service; full
reorg/clear/save/template matrix passes; fault injection exposes only typed
outcomes or authority-local reset.

### P4 — Effect journal simplification

- Replace population formulas/reservation IDs with preallocated slot/byte
  ordinary/critical static regions sized by bounded plan cohorts and reset
  effects; capacity is an exact Plan predicate, not a held credit.
- Isolate endpoint hangs/panics/retries and preserve critical headroom/FIFO.
- Delete generic EffectOutbox state and journal-triggered fail-stop.

Exit: exact charge model, saturation/restart/endpoint tests pass; no state lock
waits for effect capacity or I/O.

### P5 — Test isolation, review guide and full correctness acceptance

- Keep production source free of inline test bodies; place unit/model/fault
  tests under dedicated test files/modules.
- Map every behavior ID to unit, contextual verifier and all tx-pool-related
  integration specs, not only `test/src/specs/tx_pool`.
- Update `REVIEW_GUIDE.md` tables with invariant, attack, expected behavior,
  source boundary, test anchors and minimum/complete commands.
- Run `cargo nextest` gates, clippy/format/manifest checks, then the complete
  `make integration` tx-pool universe and classify every failure as product bug,
  deliberate policy change or stale test before changing code.

Exit: no unmapped historical finding, no test/guide drift, production source
meets the size gate or has a documented hard-proof exception.

### P6 — Performance acceptance (explicit instruction only)

- Use the saved checkpoint and develop worktrees for repeated A/B.
- First audit benchmark correctness/noise; run quick, optimize only measured
  regressions, then broader profiles as separately authorized.
- Attribute lock hold, allocations/RSS, resolve/verify throughput, commit
  latency, template latency and hostile fairness; never trade away hard safety
  bounds to improve a mean.

Exit: no material throughput/latency/RSS regression, or the architecture does
not qualify as production-ready. Until the user authorizes this phase, it
remains open and no benchmark process is started.

## 26. Permanent documentation contract

The final repository documentation has distinct roles:

- `ARCHITECTURE.md`: why this design is necessary, the formal model, rejected
  alternatives, mechanism ledger, attack/failure/performance proofs and hard
  change rules;
- `pipeline.md`: implementation/migration status and current code mapping;
- `REVIEW_GUIDE.md`: test-driven reviewer checklist linked to the exact design
  sections and behavior IDs;
- `security-regression-ledger.md` and manifest: historical evidence and machine
  checked test anchors;
- `test-inventory.txt`: complete unit/contextual/integration command inventory.

`ARCHITECTURE.md` and `REVIEW_GUIDE.md` must link to each other. A PR changing a
state, authority, queue/index, displacement cause, resource equation, lock
order, effect, reorg/assembler protocol or failure domain must update the
corresponding design row and regression row. CI treats a stale/missing link or
anchor as a review-contract failure.
