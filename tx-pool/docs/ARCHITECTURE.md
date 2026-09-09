# Transaction-pool architecture

The pool is built around **original observations → Plan → Apply → publication**.
This gives admission, replacement and chain changes one rule: validate the facts
that justified a decision, then commit all coupled changes together. Shards make
compatible commits concurrent; the outbox retains effects that must follow them.

## The architectural core

```mermaid
flowchart TB
    R["Read current facts<br/>Owner identity · Dependencies · Lifecycle"] --> P["Prepare a Plan<br/>Original reads + intended edits + required effects"]
    P --> A["Apply<br/>Validate and commit the coupled change"]
    S["Keyed shard guards<br/>Protect this Plan's read/write support"] -.-> A
    A --> O["Current owners and projections"]
    A --> E["Committed effect obligations"]
    E --> U["Publish after guards open<br/>Callbacks · Relay · Records · Estimator"]
```

| Concept | Responsibility |
|---|---|
| Read set | Retains the original premises of a decision, including absence and complete relation reads |
| Plan | Describes before/after owners, lifecycle changes and required effects; preparation is discardable |
| Apply | Revalidates premises and reserves exact capacity before committing owners, indexes, queues, charges and obligations |
| Shards and gates | Protect conflicting facts during Apply; they are concurrency machinery, not independent policy authorities |
| Effect outbox | Preserves ordered obligations after commit until publication settles |

A transaction has one current immutable owner. Raw hash is its lookup key; the
`Arc<Entry>` instance distinguishes successive owners at that key. Witness hash
identifies VM payload. Expensive resolution, verification, graph decisions and
external calls stay outside owner guards.

## One decision through the core

```mermaid
sequenceDiagram
    participant W as Worker / request
    participant S as Store
    participant P as Publisher
    W->>W: Prepare this transition
    W->>S: Plan + original observations
    S->>S: Reserve effects, acquire shard locks
    S->>S: Validate reads and capacity
    alt Stale or refused
        S-->>W: No mutation
    else Valid
        S->>S: Commit facts + obligations
        Note over S: Release guards
        S->>P: Activate committed batch
        P->>P: Call endpoints, settle batch
    end
```

A queued transaction crosses this boundary more than once: ingress installs its
first owner, resolution commits Verify or Waiting, and admission commits Accepted.
Queue selection and computation themselves do not change owner phase.

For replacement, the Plan includes both candidate and victims. If an observed
spender or dependency changed, Apply rejects the stale Plan; no partial victim
removal becomes visible. Two compatible Plans can hold their final commit guards
at the same time, including when their transactions only share a read-only cell-dep.
Publication is ordered, but is not performed inside those guards.

## Facts, owners and projections

```mermaid
flowchart TB
    O["Store.shards<br/>Raw hash → current owner"] --> E["Immutable Entry<br/>Transaction · Arrival · Source · Phase"]
    V["Store.view<br/>Chain snapshot + lifecycle revision"] -.-> P["Query and template captures"]
    E -.-> P
    E -.-> I["Derived membership indexes<br/>Relations · Peers · Proposals · Expiry"]
    E -.-> Q["Resolve / Verify scheduling"]
```

The owner is the retained transaction fact; indexes locate and classify it.
Source records permissions/provenance, while phase records its current work or
accepted/recovery state. A phase change creates a new owner instance. The paired
view supplies chain context and invalidation. Public pending/gap/proposed position
is a projection against that view, distinct from the owner's admission phase.
[Entry and Source](../src/authority/model.rs) define these facts.

Decision modules prepare Plans; **Store alone writes live owners and their
membership projections**. Pool owns jobs, computation admission and request
coordination. Block verification takes priority over pool computation while
reconciliation and publication remain live. The builder owns startup and task
joins. Jobs, suspended VM state, active reservations and publisher endpoints have
their own lifetimes, explained in [execution](architecture/EXECUTION.md).
Consensus and VM truth remain in the canonical verification crates.

## Responsibilities in code

| Change or review target | Entry point |
|---|---|
| Request protocol and completion | [controller](../src/service/controller.rs), [message](../src/service/message.rs), [dispatch](../src/service/dispatch.rs) |
| Active jobs and direct requests | [Pool](../src/authority/service.rs), [execution](../src/authority/service/execution.rs), [submission](../src/authority/service/submission.rs) |
| Policy preparation | [ingress](../src/authority/ingress.rs), [membership](../src/authority/membership.rs), [chain](../src/authority/chain.rs) |
| Coupled mutation and resource ownership | [Store](../src/authority/store.rs), [budget](../src/authority/budget.rs) |
| Resolution and canonical checks | [jobs](../src/authority/jobs.rs), [verification](../src/verification.rs) |
| Effects and projections | [notice](../src/authority/notice.rs), [query](../src/authority/query.rs), [template](../src/authority/template.rs), [packing](../src/authority/packing.rs) |
| Startup, shutdown and disk format | [builder](../src/service/builder.rs), [persistence](../src/persisted.rs) |

## Read the details

- [Commit and concurrency](architecture/COMMIT.md): read/write support, preflight,
  atomic mutation, lock order, semantic versus physical conflicts, and optimizations
  within the commit mechanism.
- [Execution and lifecycle](architecture/EXECUTION.md): owner phases, scheduling
  and block priority, job/resource lifetimes, publication, reorg, templates,
  startup and shutdown, and optimizations that reduce retained data or repeated work.
- [Review guide](REVIEW_GUIDE.md): representative tests and how to assess a change.
- [Maintenance](MAINTENANCE.md): configuration, caller migration and diagnosis.

The optimization sections explain current mechanisms, prerequisites and costs.
Their magnitude requires the source-bound [performance report](PERFORMANCE.md).
