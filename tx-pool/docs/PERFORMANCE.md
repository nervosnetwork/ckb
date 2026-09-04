# Tx-pool performance contract

Performance is part of correctness: independent CKB transactions must retain
independent progress. Timing cannot excuse a consensus, safety, resource or
recovery defect, and a green correctness suite cannot excuse global ordinary
serialization.

Hard constraints are owned by
[`architecture-contract.json`](../architecture-contract.json). Exact source and
gate results are generated under [`control/txpool-v8/`](../control/txpool-v8/).
Historical measurements remain in Git; only measurements bound to the final
reviewed source may support acceptance.

## Required behavior

- Resolution, script verification and immutable computation run outside
  mutation cuts.
- Independent owner transitions can overlap through exact shard support.
- Transactions sharing only a read-only `cell_dep` remain concurrent.
- RBF, dependency producer changes, resource capacity, scheduler selection,
  effect FIFO and chain/generation changes pay only their necessary conflict
  cost.
- No lock spans `.await`, external I/O, script execution, attacker-sized
  allocation, destruction or a population scan.
- Counts, bytes, edges, causal closure, fanout, queued work, retries and tasks
  remain bounded.
- External effects and template construction consume committed receipts after
  authority release.
- A cache or index cannot add policy, ownership, an unbounded retry path or a
  new shutdown obligation merely to improve a benchmark.

The important cost is work per committed transition, not the number of named
types. For each workload measure:

| Dimension | Observation |
|---|---|
| useful throughput | accepted transactions per monotonic target interval |
| latency | completion distribution, including p99 |
| CPU | target-window process CPU, separate from wall time |
| allocation | calls and bytes in an explicitly allocation-enabled run |
| authority | lock/stage wait and hold spans, transition counts |
| scheduling | Ready attempts/work, wakes, task polls and permit use |
| dependency/RBF | bounded edge and closure work |
| lifecycle | reorg overlap and shutdown latency |

## Evidence stack

Evidence has three layers, in this order:

1. Static ownership and bounded-work review establishes that a path can be
   safe and independently concurrent.
2. Event-driven tests observe exact production seams for atomicity,
   capability return, rollback and overlapping disjoint cuts.
3. Fixed-binary paired benchmarks measure the final source; profiles explain
   the measured cost but do not select a winner by themselves.

Deadlock detectors, stack dumps and flame graphs are diagnostics. A timeout
only bounds a hang. Concurrency proof requires barriers, channels or explicit
state witnesses showing both operations inside the production region.

## Workload matrix

The maintained executor covers these families:

| Family | Required variation | Claim |
|---|---|---|
| independent always-success | cold/warm, 1/4 peers, 1/8 workers | authority/scheduler ceiling |
| independent secp256k1 | 1/4 peers, 1/8 workers | useful VM parallel scaling |
| dependency chain/forest | parent/reverse order, shallow/deep | causal progress and bounded frontier |
| shared `cell_dep` readers | multiple peers/workers | read sharing does not serialize |
| fan-in/fan-out | bounded widths and reverse arrival | edge and wake cost |
| RBF/conflict | accept, reject, winner failure, recovery | atomic replacement cost |
| pool pressure | resource and effect limits | bounded backpressure and cleanup |
| reorg in flight | retained work plus chain update | generation convergence |
| callback delay | bounded endpoint delay | publication cannot own mutation progress |
| shutdown | active work/effects | capability return and bounded join |

Fixture generation is deterministic from scenario parameters and runner source.
Any randomized extension must bind its generator, seed and corpus identity.

## Fixed-binary comparison

[`BENCHMARK.md`](BENCHMARK.md) describes the commands. The paired runner is the
authority for its CLI and record schema. It must:

- freeze clean baseline and candidate roots before observing results;
- use byte-identical harness/workload sources and compatible consensus inputs;
- build each side once with the final `prod` profile and record binary hashes;
- use isolated equal-shape target/worktree paths;
- alternate adjacent AB/BA order;
- verify corpus and terminal identities for every attempt;
- record raw attempts atomically so an interrupted run can resume;
- aggregate only predeclared successful pairs;
- apply the declared relative-MAD/noise rule before ranking a scenario.

Allocation-observation builds are diagnostic and cannot rank timing. A noisy,
incomplete or unexplained scenario remains unresolved; it does not authorize a
cache, delay, retry loop or second authority.

Acceptance requires no material regression on the representative matrix and
no loss of independent scaling. A geometric summary cannot hide a failed
required scenario. Adversarial shapes also need absolute operation counts when
a throughput ratio cannot expose asymptotic work.

## Profiling

`scripts/profile.py` captures the same production-shaped one-shot executor with
Samply. The benchmark emits a monotonic target window; analysis crops samples
to it and reports deterministic leaf/inclusive hotspots plus authority span
counters. The movable bundle binds source, binary, workload, environment and
artifact hashes.

Profiles answer *where* time moved. Paired A/B answers *whether* the final
candidate changed performance. Whole-process RSS, context switches and CPU are
useful diagnostics but do not replace target-window observations.

Optional node telemetry must degrade locally when unavailable or misconfigured;
it is never a startup or consensus precondition.

## Gates

After changing the executor or analysis surface:

```sh
cargo check -p ckb-tx-pool --bench profile_one_shot --features profiling,allocation-observation
python3 -m unittest tx-pool/scripts/test_profile.py
python3 -m unittest tx-pool/scripts/test_cross_version_benchmark.py
```

Before final acceptance, run the repository build/test/integration gates and
the predeclared paired matrix against the exact final source. Record source,
binary, workload, environment, raw samples and the noise decision together.

## Reviewer test

For every performance mechanism ask:

1. Which measured work does it remove?
2. Does it preserve the minimal conflict rule and shared-`cell_dep` concurrency?
3. Where are ownership, bounds, cancellation and shutdown encoded?
4. Which production-seam test proves the safety claim?
5. Which final-source paired result proves the benefit?

If the mechanism only moves work to another queue, cache, task or failure path,
or lacks a stable measured benefit, remove it.
