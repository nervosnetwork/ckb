# Tx-pool profiling contract

Profiling is a maintained, feature-gated development component. It attributes
cost in one exact source, binary, workload and host identity. It does not prove
correctness, a static lower bound, product dominance or the global goal.

## Maintained entry points

`tx-pool/scripts/profile.py capture` samples the Criterion pipeline harness.
It varies transaction implementation (`always_success` or `secp256k1`), a
single dependency chain in either submission order, cold or warm pool state,
peer count, worker count and target size.

`tx-pool/scripts/profile.py capture-one-shot` samples the production-shaped
one-shot harness. Its scenario grammar covers:

- independent `always_success` and `secp256k1` transactions;
- parent-first and child-first single chains;
- `dependent_forest_<depth>` and
  `dependent_forest_<depth>_reverse`;
- `always_success_fanin_<width>`;
- `fanout` and `fanout_reverse`;
- `reorg_in_flight`;
- `always_success_callback_<microseconds>us`.

Reverse dependency scenarios require `warm=0`. A prefix of a child-first
chain/forest/fanout is not an accepted warm pool because its parents have not
arrived; the harness rejects that configuration instead of waiting for an
impossible warm terminal count.

Both entry points use real `submit_remote_txs` batches. One peer contributes
one already-bounded batch and consumes one controller queue capability; peer
batches may be in flight concurrently. A sequential series of
`submit_remote_tx` calls is not an admitted production-shape profile.

The one-shot harness is also the source used by
`tx-pool/scripts/cross_version_benchmark.py`. Profiling attributes a frozen
candidate before comparison; the cross-version runner owns paired randomized
performance adjudication.

Both profiling capture entry points and the cross-version runner build their
benchmark binaries with Cargo profile `prod`, matching the shipped LTO and
single-codegen-unit configuration. A binary supplied to either tool must also
carry its explicit `prod` profile attestation; the tools record it with the
binary SHA-256. Results from Cargo profile `bench` remain diagnostic and cannot
select or retire a production candidate.

## Evidence boundary

Every capture produces one movable bundle containing the raw Samply profile,
presymbolication sidecar, stdout/stderr, an independent span execution and a
manifest. Artifact paths are relative to that manifest. The binary need not be
retained, but its size and SHA-256, source revision and dirty-content identity,
Cargo inputs, harness/analyzer hash, build command, feature set, toolchain,
CPU, OS, filesystem, battery and thermal observations are bound in the
manifest.

The target window begins immediately before production-shaped submission and
ends only after exact terminal callbacks have quiesced. Fixture construction,
cycle discovery, warm-up, shutdown and ordinary post-window reorg work are not
CPU-attributed to the target. `reorg_in_flight` deliberately includes the
overlapping reorg in its target window.

The default Samply rate is 1 kHz. A local 1 kHz versus 10 kHz calibration on
the same 4,096-transaction production-batch workload improved aggregate CPU
clock coverage only marginally while materially increasing wall-clock
perturbation, and it did not remove async interval-end stack attribution.
Higher rates therefore require a new workload-specific pilot and are not a
generic accuracy upgrade.

The CPU capture and low-overhead span capture are separate executions. Span
counts and lifetimes are control-flow and contention attribution, not timing
samples from the CPU execution. The analyzer crops CPU samples to the exact
window, validates absolute or delta Samply time coordinates, resolves the
presymbolication sidecar, and rejects missing, changed or structurally invalid
evidence. Complete-interval `threadCPUDelta` is summed and cross-checked against
the harness process CPU clock. It is associated with the stack sampled at the
end of the interval, so its symbol ranks are hotspot candidates rather than
exact async CPU attribution. Raw stack-sample counts are retained separately
as sampled residency/wait-state evidence; a sleeping leaf such as
`__psynch_cvwait` is not a CPU hotspot merely because it has many samples or
inherits CPU used earlier in the interval.

One-shot observations additionally bind:

- monotonic target elapsed time and throughput;
- process user, system and total CPU time within the target window;
- target-completion p99 measured from the common target start;
- allocation calls and cumulative allocated bytes within the target window;
- reorg overlap/latency and bounded shutdown latency;
- authority wait/hold and lifecycle span counts and lifetimes.

Cumulative allocated bytes are allocation traffic, not peak resident memory.
The paired comparison runner separately records process peak RSS and context
switches. A large fixed CKB-VM allocation therefore motivates a reuse
feasibility check only after CPU/RSS evidence shows material benefit and a
complete reset/ownership contract exists.

## Required checks

Run the deterministic analyzer canaries after changing the analyzer, harness
or manifest format:

```sh
python3 -m unittest tx-pool/scripts/test_profile.py
python3 -m unittest tx-pool/scripts/test_cross_version_benchmark.py
```

They require absolute and delta time coordinates to agree, reject a
non-monotonic time series, reject artifact substitution, reanalyze a moved
bundle after the capture binary is gone, validate span lifetime records and
reject one-shot workload identity drift.

Before treating a capture as causal evidence, also require:

1. the exact workload reaches its expected terminal count;
2. the raw profile has samples inside the target window;
3. required production spans are present for the exercised path;
4. the deterministic analyzer reproduces the summary from the manifest;
5. a negative production-shape canary fails if the harness is changed back to
   sequential per-transaction RPC submission;
6. the complete bundle is copied into durable, content-addressed evidence.

Profiles guide candidate engineering. Final performance claims require the
separately frozen three-way `develop` / current branch / candidate comparison,
precommitted scenarios, noise rule and practical non-inferiority margins.
