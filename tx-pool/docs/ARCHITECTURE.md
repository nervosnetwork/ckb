# Tx-pool architecture

This document explains the design a reviewer must maintain. Hard compatibility
and resource constraints live in [`architecture-contract.json`](../architecture-contract.json);
live source identity and validation results are generated under
[`control/txpool-v8/`](../control/txpool-v8/). Historical experiments remain in
Git and are not part of the architecture.

## State model

The pool is one state machine whose durable logical state is:

```text
Pool = (generation, chain view, owners, resources, dependencies,
        scheduler, indexes, effects)

Owner = PreAccepted | Accepted | ReplacementHistory
```

`owners`, keyed by raw transaction hash, is the only transaction-lifecycle
authority. Resources, dependencies, scheduler rows, indexes and effects are
projections committed with the owner transition; none may independently decide
policy. Checked-out work is a linear capability, not a second owner.

The externally meaningful transitions are:

```text
admit       nowhere/history -> preaccepted
checkout    queued          -> computing + work capability
settle      computing       -> queued | ready | terminal
accept      ready           -> accepted
remove      owner           -> nowhere/history
chain       owners + chain view -> coherent successor cut
reset       complete generation -> fresh generation
publish     committed effect -> acknowledged effect cursor
```

Every transition preserves owner, resource, dependency, scheduler, index and
effect agreement. A transition either commits that complete cut or has no
visible effect. Validation and planning do no I/O; Apply revalidates freshness
and performs the smallest atomic mutation; external effects run after authority
release and cannot veto the commit. These phases are semantic requirements,
not a requirement to retain particular Rust type names.

## Minimal conflict rule

Concurrency is derived from transition support, not from the existence of a
shard type. For transition `T`, define the state facts it reads and writes as
`R(T)` and `W(T)`. Two transitions may commit concurrently exactly when:

```text
W(A) ∩ (R(B) ∪ W(B)) = ∅
W(B) ∩ (R(A) ∪ W(A)) = ∅
```

The implementation may conservatively widen support only where a bounded fact
cannot be known before the cut. It must not turn that uncertainty into a global
ordinary mutation lock.

Necessary conflicts include:

- the same raw-hash owner or owner version;
- input spends and RBF membership affected by the transition;
- a producer whose availability or loss changes a consumer's evidence;
- bounded aggregate or peer resource capacity being reserved or released;
- the scheduler frontier that selects or retires the affected work;
- the FIFO/reset region of an emitted effect;
- chain, ruleset or generation replacement.

Two transactions that only read the same `cell_dep` do not conflict. That
shared reference must remain concurrently admissible. A transition that
removes or changes the referenced producer does conflict with its readers
because their dependency evidence changes.

## Physical realization

The current implementation routes owner-scoped facts over 64 physical shards.
An ordinary transition holds the shared lifecycle generation and acquires one
canonically ordered exact shard cut. Disjoint cuts can overlap; no ordinary
route may fall back to an outer write lock. The exclusive lifecycle arm is for
chain/generation replacement and close, where the state algebra itself is
global.

Some facts have short dedicated linearization points because their identity is
shared: resource reservation, scheduler selection, dependency publication and
the effect FIFO. Each such cut owns one named fact, has bounded work and is
released before I/O. Combining them into a renamed global mutation authority
would violate the concurrency contract.

```text
bounded input -> validate -> immutable computation -> plan support
              -> freshness check + exact atomic cut -> committed receipt
              -> release authority -> I/O/publication
```

The 64-shard layout is an implementation choice, not the proof. Its proof is
that exact support implements the conflict rule above and permits overlapping
independent commits.

## Linear capabilities and failure

Move-only values carry obligations that must not be reconstructed from ambient
state:

- checked-out work owns its exact owner version and execution permit;
- a settlement returns the exact result or rejection for that work;
- a staged resource/effect/dependency reservation must commit or roll back;
- a publication receipt owns one immutable effect cursor;
- a chain command returned after a failed Apply remains the same command.

Stale evidence is ordinary optimistic contention. Capacity exhaustion is typed
backpressure. Structural disagreement between coupled projections is an
authority fault. Endpoint, encoding and local scheduling failures do not become
consensus facts or peer-ban evidence.

No authority cut spans `.await`, storage/network I/O, script execution,
attacker-sized allocation, destruction or a population scan. Counts, bytes,
edges, fanout, retries, queued work and task ownership are bounded. Shutdown
cancels, joins and returns every capability before persistence is allowed.

## Main paths

- **Remote/Proposal/Recovery:** validate outside authority, commit the longest
  bounded homogeneous ingress prefix, then wake compute.
- **Local:** resolve and verify outside mutation cuts, then use the same final
  membership and RBF policy as retained work.
- **Compute:** one bounded exchange assigns a compatible wave; resolve and VM
  verification remain parallel; settlement commits exact returned
  capabilities.
- **Ready:** strict deterministic policy selects a bounded compatible prefix;
  permanent workers commit independent exact cuts concurrently.
- **Dependency:** availability/loss is a derived keyed relation. Hidden staged
  rows publish with their owning transition and cannot become visible early.
- **Effects:** one bounded FIFO/reset log records post-commit obligations.
  Publishers borrow immutable receipts; settlement advances the exact cursor.
- **Chain/reset:** the ordered global cut reclassifies or removes affected
  owners and publishes all coupled projection changes together.
- **Reads/template/persistence:** consumers capture coherent owned receipts,
  release authority, and perform expensive work afterward.

## Rust boundary

Enums and private constructors encode lifecycle states. Linear capabilities
encode commit-or-return obligations. Exhaustive matches keep new states from
silently entering old paths. Core code uses domain `Result`/`Option`; a panic or
unchecked operation requires a local proof that the state is unreachable.

A type is justified only if it prevents an illegal state, owns a linear
capability, or names a stable public boundary. A wrapper that merely renames a
field or duplicates another transition's state is removable. The same test
applies to `Plan`, `Prepared`, `Ready` and every other implementation concept.

## Review map

Read in this order:

1. `state.rs`, `work.rs`, `ingress.rs`: owners and boundary capabilities.
2. `plan.rs` and `plan/`: transition compiler and Apply cuts.
3. `shard.rs`, `resources.rs`, `dependency.rs`, `scheduler.rs`, `effect.rs`:
   support and coupled projections.
4. `runtime.rs`, `worker.rs`, `service.rs`: orchestration and lifecycle.
5. `read.rs`, `query.rs`, `template.rs`, `publisher.rs`: consumers of committed
   receipts.

For every new state, type or lock, ask:

1. Which externally meaningful transition requires it?
2. Which illegal state or capability loss does it prevent?
3. What exact conflict fact requires serialization?
4. Why can the existing transition compiler not express it?
5. Which test observes the claim on the production seam?

If those questions have no concrete answer, the concept is not part of the
minimal architecture.
