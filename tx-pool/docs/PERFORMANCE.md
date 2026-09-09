# Transaction-pool performance report

**Final delivery study completed on 2026-09-10, with exclusions.** A/A and develop A/B retained 2,600 executions and passed independent record/arithmetic audits. Three of eight rows qualify for directional comparisons. Both runners exited 2 because some rows failed the declared quality or terminal gates; successful audit does not turn those rows into passes.

The qualified results show a workload-dependent tradeoff. With eight workers, `always_success` throughput is 2.9684× develop, with 18.18% more target-window CPU and 8.85% higher process peak RSS. `secp256k1` throughput is 1.90% lower, with CPU 3.24% higher and RSS 9.47% higher. The in-flight reorg workload has 4.10% higher throughput and 32.74% lower CPU, with 11.12% higher RSS. Percentages use medians of paired ratios.

This is not evidence of an across-the-board improvement. Five rows cannot support formal performance directions in this study. The [benchmark guide](BENCHMARK.md#aa-and-final-delivery-protocol) owns the prospectively declared rules; [profiling](PROFILING.md) describes separate attribution methods.

## Frozen inputs

| Identity | Value |
|---|---|
| Candidate production and prepared commit | `f9c6843ca91d95e96b4d6ad29aab5bb08fed0bc4` |
| Develop production basis | `cdfde29e45dfbe9443be66083241ebc1acca9fe6` |
| Develop prepared measurement commit | `af398a174af432c92ca456a856a4c7504412a282` |
| Study identifier | `final-delivery-cdfde29e-r1` |
| Execution manifest SHA-256 | `1be516dbb66904f84754d95a27d15e4a39e5b8aebe47aafa33e73bc17bb10b43` |
| Host | macOS-26.6.2-arm64-arm-64bit-Mach-O; 18 logical CPUs |
| Toolchain | Rust 1.95.0 / Cargo 1.95.0; aarch64-apple-darwin; Python 3.14.7 |
| Build | `prod`; profiling, Console and allocation observation disabled |
| CKB-VM | `ckb-vm` and `ckb-vm-definitions` 0.24.15; identical locked packages and enabled features |

Develop production Rust is unchanged by the measurement adapter. Its patch contains only `Cargo.lock`, `tx-pool/Cargo.toml` and the three shared harness files. Both sides use byte-identical harness files; legacy ingress submits transactions individually and candidate ingress uses the production bounded batch interface. The two binaries were built serially after invalidating every local workspace package, and copied before the next build. No local source artifact was reused.

| Binary | SHA-256 |
|---|---|
| candidate | `01d462ecb55320fc15279b15f8587fbe108be857c7e1d7a6f1341d4025fdc810` |
| develop | `f111343f363520c34555a6a9128e552be9f5d585dcb72baa2ab71271ea7ef2bb` |

| Measurement source | SHA-256 |
|---|---|
| tx-pool/benches/profile_one_shot.rs | `36f098fb626669819567322d84ba74b363c26af743af9aa51b696402476e8769` |
| tx-pool/benches/profile_spans/mod.rs | `66170faff688cac499c827724ca44c8ca1d7074645aa99cb697d2fbcfae2dc50` |
| tx-pool/benches/relay_batches/mod.rs | `6098d040dd9efa62b9c1937a723c288747057bae8c94f195a23456bfe4215953` |
| cross_version_benchmark.py | `815cfd03a7b7d5681a5676a225acbce511c7f1439e5f6b5a67b31bf788322816` |
| measurement_process.py | `801fdeba50cf1d925e02c3d7a5826ba340a6014aea97997e06c17a0fa38e0463` |

Native engineering validation covered 405 passing Nextest tests (two skipped), strict workspace Clippy, all 176 release integration cases and generated RPC documentation. The final Python metadata stream correction passed 51 measurement-script tests; Rust inputs were unchanged after the native checks. This report adds no cross-platform CI evidence. The measured candidate commit stays the reference even if a later documentation-only commit contains this report.

## Method and full matrix

A/A ran 2026-09-10 06:06:37–08:53:31 Asia/Shanghai; A/B ran 08:53:31–11:26:37. They retained 1,393 and 1,207 attempts respectively: 2,593 successful executions and seven rejected executions. The maximum declared population was 1,552 attempts per study. Unexecuted samples after a failed row are not silently replaced.

Every row prescribed one pilot per side and 24 pairs, each containing four fresh executions per side. Side order alternated within the pair. Controls were a 30-second initial cooldown, five seconds after every attempt, a 180-second outer attempt timeout, a 0.25-second minimum individual target window, throughput-ratio MAD at most 1.5%, and primary ratio interval relative width at most 4%. The harness also has a 120-second completion wait. A/A requires all three primary intervals inside [0.98, 1.02]. A/B directions require the matching A/A row, complete A/B quality and the metric interval excluding one.

Intervals below are conservative exact-binomial order-statistic intervals for the median paired ratio, with requested 95% pointwise confidence. For 24 pairs the selected ranks are 7 and 18 (97.734% coverage under stable independent sampling). Neither balanced ordering nor these intervals prove sampling independence or simultaneous coverage of the matrix.

| Scenario / workers | Target / warm | Peers | A/A pairs; outcome | A/B pairs; outcome | Formal directions |
|---|---|---|---|---|---|
| always_success / 1 | 16,000 / 1,000 | 4 | 22/24; Clock mismatch | 14/24; Clock mismatch | No |
| always_success / 8 | 16,000 / 1,000 | 4 | 24/24; Qualified | 24/24; Qualified | Yes |
| secp256k1 / 1 | 4,000 / 100 | 4 | 17/24; Clock mismatch | 17/24; Clock mismatch | No |
| secp256k1 / 8 | 4,000 / 100 | 4 | 24/24; Qualified | 24/24; Qualified | Yes |
| dependent_forest_10 / 8 | 16,000 / 1,000 | 4 | 24/24; Qualified | 21/24; Clock mismatch | No |
| fanout_reverse / 8 | 5,752 / 0 | 4 | 11/24; Relay reset | 0/24; Baseline pilot timeout | No |
| rbf_pairs / 8 | 16,384 / 16,384 | 4 | 24/24; RSS interval too wide | 24/24; Quality passed | No; descriptive only |
| reorg_in_flight / 8 | 2,000 / 100 | 4 | 24/24; Qualified | 24/24; Qualified | Yes |

## Primary measurements

Absolute values are medians across the 24 four-execution side samples. A sample pools throughput as total target transactions divided by summed target elapsed time, sums target-window process CPU, and takes the maximum process-lifetime peak RSS. CPU seconds below therefore cover four executions, not one; divide by four to obtain the average per execution. RSS is MiB (2²⁰ bytes), including setup, warmup and shutdown. It is not a tx-pool-owned-memory measurement.

Ratios are medians of within-pair candidate/develop ratios, not ratios of the two displayed medians. Throughput benefits from higher values; CPU and RSS from lower values. `—` means no complete-row estimate: incomplete prefixes remain in the raw packet but are not summarized into a substitute result. `†` marks the complete RBF A/B observation whose failed A/A gate prevents formal ranking.

### Throughput

| Scenario / workers | Develop (tx/s) | Candidate (tx/s) | Candidate / develop [interval] |
|---|---|---|---|
| always_success / 1 | — | — | — |
| always_success / 8 | 21,112.90 | 62,704.56 | 2.9684 [2.9598, 2.9830] |
| secp256k1 / 1 | — | — | — |
| secp256k1 / 8 | 8,140.26 | 7,984.21 | 0.9810 [0.9788, 0.9819] |
| dependent_forest_10 / 8 | — | — | — |
| fanout_reverse / 8 | — | — | — |
| rbf_pairs / 8 † | 50,056.86 | 49,608.68 | 0.9923 [0.9903, 0.9969] |
| reorg_in_flight / 8 | 1,257.11 | 1,309.67 | 1.0409 [1.0403, 1.0427] |

### Target-window process CPU

| Scenario / workers | Develop (CPU seconds / 4 executions) | Candidate (CPU seconds / 4 executions) | Candidate / develop [interval] |
|---|---|---|---|
| always_success / 1 | — | — | — |
| always_success / 8 | 8.591705 | 10.164616 | 1.1818 [1.1773, 1.1849] |
| secp256k1 / 1 | — | — | — |
| secp256k1 / 8 | 16.229890 | 16.753979 | 1.0324 [1.0312, 1.0336] |
| dependent_forest_10 / 8 | — | — | — |
| fanout_reverse / 8 | — | — | — |
| rbf_pairs / 8 † | 6.641757 | 12.748278 | 1.9145 [1.9093, 1.9214] |
| reorg_in_flight / 8 | 2.115832 | 1.424751 | 0.6726 [0.6646, 0.6827] |

### Process-lifetime peak RSS

| Scenario / workers | Develop (MiB) | Candidate (MiB) | Candidate / develop [interval] |
|---|---|---|---|
| always_success / 1 | — | — | — |
| always_success / 8 | 202.227 | 219.867 | 1.0885 [1.0756, 1.1026] |
| secp256k1 / 1 | — | — | — |
| secp256k1 / 8 | 151.141 | 165.594 | 1.0947 [1.0934, 1.0982] |
| dependent_forest_10 / 8 | — | — | — |
| fanout_reverse / 8 | — | — | — |
| rbf_pairs / 8 † | 219.547 | 250.328 | 1.1337 [1.1237, 1.1498] |
| reorg_in_flight / 8 | 114.852 | 127.578 | 1.1112 [1.1096, 1.1131] |

RBF’s descriptive A/B medians show a throughput ratio of 0.9923, CPU ratio of 1.9145 and RSS ratio of 1.1337. These resource observations are retained, but the missing A/A qualification prevents a formal RBF direction. Its A/A RSS interval was [0.977618, 1.024041], with 4.636% relative width: it failed both the 4% precision rule and the ±2% equivalence margin.

## Ancillary observations

P99 is the maximum of four per-execution target callback-latency p99 values within each sample, then the median across samples. It is not a pooled transaction percentile or network latency. Reorg and stop are separately timed controller observations, not additive target phases. The primary A/A gate does not establish ancillary A/A equivalence.

### Callback P99

| Scenario / workers | Develop (ms) | Candidate (ms) | Candidate / develop [interval] |
|---|---|---|---|
| always_success / 1 | — | — | — |
| always_success / 8 | 759.232 | 253.477 | 0.3341 [0.3329, 0.3350] |
| secp256k1 / 1 | — | — | — |
| secp256k1 / 8 | 487.355 | 496.390 | 1.0184 [1.0156, 1.0213] |
| dependent_forest_10 / 8 | — | — | — |
| fanout_reverse / 8 | — | — | — |
| rbf_pairs / 8 † | 325.514 | 328.527 | 1.0062 [1.0017, 1.0114] |
| reorg_in_flight / 8 | 1,580.492 | 1,514.006 | 0.9590 [0.9556, 0.9600] |

### Reorg request return

| Scenario / workers | Develop (ms) | Candidate (ms) | Candidate / develop [interval] |
|---|---|---|---|
| always_success / 1 | — | — | — |
| always_success / 8 | 0.006208 | 17.100438 | 2850.5359 [2606.5334, 3686.4318] |
| secp256k1 / 1 | — | — | — |
| secp256k1 / 8 | 0.004729 | 3.427625 | 740.2294 [672.4472, 786.5521] |
| dependent_forest_10 / 8 | — | — | — |
| fanout_reverse / 8 | — | — | — |
| rbf_pairs / 8 † | 0.006854 | 20.249522 | 2944.4079 [2779.4912, 3282.9484] |
| reorg_in_flight / 8 | 0.002105 | 7.615500 | 3211.2816 [908.4496, 5537.5455] |

The develop controller returns after `reorg_sender.try_send`; the candidate performs synchronous reconciliation work before returning. In ordinary rows this request is after the target window; `reorg_in_flight` issues it while submissions/callbacks are active. These numbers measure different return boundaries and cannot rank completed reorg latency. The reorg workload’s primary throughput/CPU numbers describe its target-terminal fixture window, not block acceptance or complete chain recovery.

### Stop observation

| Scenario / workers | Develop (ms) | Candidate (ms) | Candidate / develop [interval] |
|---|---|---|---|
| always_success / 1 | — | — | — |
| always_success / 8 | 0.860541 | 1.011980 | 1.1992 [0.9334, 1.6760] |
| secp256k1 / 1 | — | — | — |
| secp256k1 / 8 | 1.352167 | 0.944416 | 0.7057 [0.6669, 0.7430] |
| dependent_forest_10 / 8 | — | — | — |
| fanout_reverse / 8 | — | — | — |
| rbf_pairs / 8 † | 0.215188 | 1.307667 | 6.0769 [5.4518, 7.9481] |
| reorg_in_flight / 8 | 1.229437 | 1.129438 | 0.9120 [0.9049, 0.9905] |

The candidate calls `stop()`, observes `service_started == false`, and joins the relay observer; service workers and publication can still be joining. Develop broadcasts exit signals and joins the observer, then exits without ordinary destructors. These times cannot rank full service join, persistence or whole-node shutdown. For example, the secp stop ratio interval [0.6669, 0.7430] describes only these narrower observations; it is not evidence that candidate node shutdown is faster.

## Every rejected execution

Each failed pilot prevented pairing for its row; each paired failure stopped that row immediately. The exact identifiers below are suffixes after `scenario-tTARGET-wWARM-vWORKERS-pPEERS/` in the retained JSON. A/A side labels refer to the same candidate binary.

| Study | Row | Attempt suffix | Rejection |
|---|---|---|---|
| A/A | always_success-t16000-w1000-v1-p4 | pair-23/replicate-3/candidate | Clock mismatch: wall − monotonic +2.010375 ms; tolerance 1 ms |
| A/A | secp256k1-t4000-w100-v1-p4 | pair-18/replicate-3/candidate | Clock mismatch: wall − monotonic +197.837833 ms; tolerance 1 ms |
| A/A | fanout_reverse-t5752-w0-v8-p4 | pair-12/replicate-3/baseline | Exit 1: relay generation_resets=1; duplicate_ok=0; duplicate_reject=0; unknown_parents=8893 |
| A/B | always_success-t16000-w1000-v1-p4 | pair-15/replicate-1/baseline | Clock mismatch: wall − monotonic +58.538459 ms; tolerance 1 ms |
| A/B | secp256k1-t4000-w100-v1-p4 | pair-18/replicate-2/baseline | Clock mismatch: wall − monotonic +1.545458 ms; tolerance 1 ms |
| A/B | dependent_forest_10-t16000-w1000-v8-p4 | pair-22/replicate-2/baseline | Clock mismatch: wall − monotonic -97.187042 ms; tolerance 1 ms |
| A/B | fanout_reverse-t5752-w0-v8-p4 | pilot/baseline | Exit 1 after ~120.186 s: accepted 101/5752 transactions, then completion timeout |

The clock gate compares independently captured wall and monotonic durations. These retained records identify the disagreements, not their operating-system cause. Clock adjustment and scheduling around the separate reads were not isolated. The threshold was not relaxed and rejected records were not converted into performance samples.

The candidate fanout execution violated the prospectively required exact relay stream. [Relay mailbox reconciliation](../src/authority/relay.rs) can produce a reset on overflow or accounting mismatch; explicit reset effects are also present in the implementation. The retained observation has no producer tag, so it does not isolate which route caused this reset or prove a consensus failure. The develop fanout pilot timed out on this supplied reverse-fanout corpus. Neither observation establishes a universal failure rate, and this row has no valid performance comparison.

## Interpretation and remaining limits

The accepted rows establish a substantial cheap-script throughput gain, a small but resolved secp throughput regression, and an in-flight-reorg fixture throughput/CPU gain. All three qualified rows use more process peak RSS. The architecture mechanisms in [the architecture guide](ARCHITECTURE.md) may explain costs or benefits, but this whole-change comparison does not isolate any one mechanism’s contribution.

Broad performance acceptance remains open. The material unresolved work is to explain the clock disagreements, characterize the fanout relay/reset and baseline completion behavior, and investigate the secp CPU/RSS cost with separately declared diagnostics. This batch was completed as frozen; no follow-on optimization, resampling or relaxed qualification was folded into it. Finite native measurements do not prove global optimality, cross-platform performance, full shutdown completion or absence of regressions outside these workloads.

## Reproduction and evidence

The accompanying `tx-pool-final-delivery-20260910-evidence.tar.gz` packet contains both complete raw JSON files, all seven rejected outputs, both independent adjudications, exact commands and exits, preflight/completion process observations, source/build identities, both production-relative patches, shared harness and measurement tools, and a portable arithmetic replay. It excludes native executable payloads; their hashes and source-bound build receipts are retained.

Extract the archive, enter its `tx-pool-final-delivery-20260910-evidence` directory and run `python3 -B replay.py`. The replay checks the packet inventory and recomputes every retained successful/failed record, pair reduction, interval and qualification without starting a benchmark. It compares the result to both archived audits. It does not rebuild binaries or independently recreate historical OS observations. The original frozen source checkouts and native binaries remain available in the local delivery evidence directory.

Evidence archive SHA-256: `f68e7b3c33d2deb1b07f8877a359b20b755cc87b99410931625b63414fa21e31`. The archive contains 103 files and is 2,637,994 bytes. A fresh extraction completed both offline replays successfully.
