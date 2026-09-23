# Commit and concurrency

[Architecture overview](../ARCHITECTURE.md) introduces the core. This page explains
how [Apply](../../src/authority/store/apply.rs) validates a prepared
[Plan](../../src/authority/store/plan.rs) and protects its coupled mutation.
[Execution](EXECUTION.md) owns the surrounding job and effect lifetimes.

## Original reads and intended writes

Preparation records the facts it actually used. Apply must validate those original
facts, not substitute newer observations that happen to support the same outcome.
This applies to a rejection as well as an acceptance.

| Observation | Why it is needed |
|---|---|
| Positive owner identity | The same transaction hash can leave and reenter; an old worker must not act on its successor |
| Absence | A missing producer/spender can appear before commit |
| Point spender value | RBF must validate the specific input conflict used in its decision |
| Complete relation or peer set | A new descendant, reader or cohort member must not escape a prepared coupled change |
| Paired lifecycle and snapshot | Reorg or same-tip clear invalidates work even when a tip identifier alone looks unchanged |

Positive reads use `Weak<Entry>` identity. They do not pin retired transaction
payloads; retained weak control blocks prevent pointer-reuse ABA. Complete relation
and peer observations also use identity markers. Different repeated observations
make a read set stale. Absence is a current predicate: absent → present → absent
history alone does not falsify it.

A Plan owns the decision from preparation through Apply. Construction seeds the
original resolution or capture reads; later tracked reads extend that same private
set. The [membership graph](../../src/authority/membership/graph.rs) borrows the
Plan and hides its Store and owner cache from membership policy. It preserves the
first owner observation, including absence, and records complete input/dep reader
and child relations even when they are empty. Capacity planning can extend the
same decision with a full accepted capture. Only full captures allocate the
optional shard-revision arrays; ordinary reads stay sparse.

`Plan::edit` adds a unique before/after owner change and its optional effect
together, after checking the old identity and duplicate/hash constraints.
Metadata-only changes explicitly pass no effect. Notice-only outcomes use
`Plan::notify` and still validate the decision's premises. Effect fields are
private: constructors define accepted, rejected, waiting, projection, ban, reset
and block obligations. Candidate refusal and existing-owner removal have separate
constructors; removal derives relay eligibility from the old owner's phase and
origin rather than requiring callers to repeat that rule. Peer bans and their
generation-reset notices share one Plan operation. Attached-block records live
inside the lifecycle write whose snapshot they accompany. Write support,
relation changes, queues and charges derive from the owner edits, without a
second mutable membership ledger.

A policy rejection consumes the existing Plan, discards its speculative edits
and effects, and constructs only its required rejection outcome. Original reads,
peer eligibility and dry-run mode survive. This keeps a late policy failure from
leaking victims, acceptance notices or lifecycle changes.

Read tracking cannot infer missing business premises. A producer admitted after
its readers must discover both input and cell-dep readers and become their parent.
Replacement needs the complete victim closure and backing; child-before-parent
rejection notices precede admission. The private interfaces preserve recorded
observations but do not prove the policy recorded everything it needed.

### Example: replacing an input spender

Suppose candidate B spends the same input as accepted A, and C depends on A.
Membership reads the current A/C owners, the input's spender and the complete
relations used to discover victims and validate backing. The Plan describes B's
admission, A/C retirement or optional history, and the required notices.

If another transaction adds a dependent reader before Apply, the original relation
observation is stale: nothing is removed. If the premises still hold and exact
capacity is available, candidate, victims, indexes, charges and notice obligations
change together. Sharding determines which guards protect those facts; it does
not choose the replacement policy.

## Preflight and commit

```mermaid
flowchart TB
    P["Plan prepared outside guards"] --> R["Derive support and charge differences<br/>Reserve complete effect batch"]
    R --> G["Acquire ordered guards"]
    G --> V{"Original reads, old owners,<br/>permissions and capacity valid?"}
    V -->|No| X["Release reservations<br/>No visible mutation"]
    V -->|Yes| Q["Reserve exact positive owner capacity"]
    Q -->|Stale or full| X
    Q -->|Reserved| C["Commit owners, indexes, queues,<br/>snapshot, charges and effect batch"]
    C --> U["Release guards<br/>Activate publication and notify"]
```

A disjoint commit can consume scalar capacity after earlier planning. Final owner
reservation can still refuse: sparse accepted-capacity pressure becomes Stale so
fee policy can be replanned, while other refusals retain their classified outcome.
The exact validated victims fund replacement credit. Speculative credit cannot
escape a refused commit. Dry-run shares membership policy and final capacity
validation, then stops without reserving notices or installing owners or effects. Capacity trimming
can enlarge the RBF victim set, so complete backing is rechecked for that final set.

A verified [admission operation](../../src/authority/service/admission.rs) keeps
its candidate, original predecessor, proof and history policy across replanning.
Direct and worker callers retain their distinct stop, stale-work and publication
completion contracts; previews use the same decision without committing.

Replacement history is optional. If only its pipeline/history capacity is lost,
`apply_admission` removes the proposed history owners and retries the same Plan
once synchronously, retaining its exact effect reservation. Every original read
and final owner charge is checked again. The reservation never escapes into an
asynchronous wait; later replanning carries the decision to omit history.

All fallible policy, arithmetic and capacity checks precede mutation.
`OwnerChanges` derives routed owner edits, relation/peer changes and wake keys
in one pass over the original edits. Store's `CommitLocks` routes read-set
guards once for Apply and for the point reads supported by guarded publication,
then adds owner writes and lifecycle operations for Apply. Its exhaustive
ReadSet destructuring makes new observation kinds declare their protection in
both paths. Under those guards, `OwnerChanges::validate_projections` checks
counters and derived indexes before scalar capacity is reserved. The derived
changes borrow the Plan's edits rather than copying owners; the same object
installs the owner, relation and peer projections after validation. One
`Application` owns the Plan and its notice reservation across the synchronous
history fallback.
`commit_infallibly` takes a unit-returning closure, so an accidental `?` in that
tail does not compile. This narrow guard does not prove preflight completeness,
prevent panics or promise rollback after a committed integrity fault. Allocation
uses Rust's abort-on-OOM behavior. A generation fault prevents continued normal
use and makes persistence ineligible.

Within that commit, `Shard::apply_edit` updates the owner and every shard-local
projection, including queue membership and retirement. Its exhaustive field
destructuring forces a new Shard field to be considered at this boundary; it does
not prove the update is correct. Clear replaces complete shards. A lifecycle
write refreshes proposed counts against its successor snapshot, installs the
snapshot and applies its ordered committed-hash records under the same cut.

Retired payloads and replaced collections drop outside the guarded cut. The
outbox batch is appended with the mutation and becomes ready after guards open.
This preserves the obligation even when a caller stops waiting; delivery to an
external endpoint has its own failure boundary.

## Semantic conflicts and physical guards

A semantic conflict exists when one operation changes a fact another relies on.
Physical routing conservatively groups facts for synchronization.

| Pair of operations | Required relationship |
|---|---|
| Spend the same input | Conflict; validate the original spender and membership policy |
| Add a reader while replacing its backing producer | Conflict with the complete backing/reader premise |
| Read the same cell-dep without changing it | Compatible |
| Admit remote work while banning that peer | Peer permission check and commit must be protected together |
| Ordinary mutation during chain/clear replacement | Conflict with lifecycle replacement |

The current Store uses randomized keyed routing over 256 owner shards and separate
peer/dependency gate families. Owner routing incorporates proposal ID so a short-ID
collision shares its owning cut. Within a physical shard, writer mode dominates
reader mode. Complete relation reads take their gate exclusively; point reader
additions share it and update the short row under its mutex.

```mermaid
flowchart TB
    V["Paired view<br/>Shared for ordinary work, writer for chain/clear"] --> P["Peer gates · ascending index"]
    P --> D["Dependency gates · ascending index"]
    D --> O["Owner shards · ascending index"]
    O --> M["Short collection, row and queue operations"]
```

Collection mutexes precede their row mutexes. Budget, outbox and dirty-key
bookkeeping must not acquire an earlier layer while held. Queue selection releases
its lane before owner lookup; maintenance releases its dirty-key mutex before
relation lookup. No authority guard spans await, storage I/O or a foreign call.

Ordinary work shares the lifecycle view and visits only its support. There is no
global membership writer around independent transactions. Physical collisions
can still serialize them, and short quota/row/FIFO operations remain. Full capture
and clear visit all owner shards. The guard bound is per family: Apply can retain
up to 768 peer, dependency and owner guards plus its lifecycle guard.

Remote admission holds the peer gate shared across its final ban check and commit;
a ban holds it exclusively. A ban winning first makes prepared work recheck. The
marker alone does not retroactively remove work committed first. Ban-marker expiry
and bounded table eviction are separate policies.

## Dependency and replacement decisions

[Jobs](../../src/authority/jobs.rs) resolve against the chain snapshot and accepted
overlay while recording producer/spender premises. A pool spend does not make
chain backing dead; [membership](../../src/authority/membership.rs) decides RBF.
Admission requires every resolved input, cell-dep, dep-group container and expanded
member to remain live after replacement. A spender already in the pool must be
removed by that same replacement; a later dependency reader is otherwise rejected.
Successful admission fixes the order: earlier readers remain accepted; a later
reader cannot use an observation made before the spender committed. Only producers
create causal ancestry. [Packing](../../src/authority/packing.rs) requires retained
earlier readers before the spender, across blocks when necessary, without adding
them to the ancestor budget.

RBF counts shared descendants once, checks full input/dependency backing, and
commits candidate plus victims atomically. Its original victim set also identifies
replacement notices; additional capacity victims receive the full-pool reason.
Complete relation observations prevent an interposed reader from escaping the
plan. Self-eviction does not evict incumbents
for a rejected candidate. Wide fee totals preserve exact descendant accounting;
optional fee overflow must not poison unrelated admission or query paths.

Membership calculation remains transient. Complete aggregate queries reuse
ordinal marks and one traversal scratch area across roots, enforcing the same
ancestor, cycle and arithmetic checks as a point walk. Removal notices calculate
totals for their actual victims; capacity trimming combines each removed closure's
contribution before updating surviving ranks. Shared descendants count once,
and no subsequent victim is chosen before those rank updates finish. These
calculations add bounded scratch, not persistent transitive membership.
