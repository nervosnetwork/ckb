# Transaction-pool performance evidence

Performance belongs to the measured executable inputs. The latest incremental
comparison below uses `670a58042824ea0665a86abab8f8535a2b1d6417` and the tracking PR branch
`54c26b2e1fe2954ade628304f471cc81e73b6f6b`. The historical develop comparison uses different
sources and must not be treated as a measurement of the current code.

[Architecture](ARCHITECTURE.md) describes the design;
[benchmarking](BENCHMARK.md) defines capture and qualification;
[profiling](PROFILING.md) covers attribution; and
[maintenance](MAINTENANCE.md) records operational contracts.

## Latest incremental comparison

Measured on macOS 26.6.2 arm64, 18 logical CPUs, Rust 1.95.0, profile `prod`,
CKB-VM 0.24.15 with matching `detect-asm` features. Profiling and allocation
instrumentation were disabled. Both sides use identical Rust benchmark sources.
Node regressions completed before measurement. Sources and frozen executable
hashes were checked before and after capture. A shared-target stale metrics
artifact caused an initial final-build failure; workspace artifacts were
invalidated and the benchmark rebuilt before any final capture.

### Network admission

Four workloads use eight workers and four simulated peers. Each has three
independent paired process runs with alternating A/B order: 24 runs total.
Both sides finish the same transaction/cycle corpus and callback/relay terminals.
Elapsed time and CPU cover submission through those terminals. RSS is the whole
process lifetime peak, including setup and cleanup. These are network admission
measurements; they do not measure local blocking RPC latency.

| Workload | Target / warm | Elapsed ms, remote → final | Time change | CPU change | Peak RSS change |
|---|---:|---:|---:|---:|---:|
| always_success | 16,000 / 128 | 248.045 → 247.317 | -0.06% | -0.55% | +0.47% |
| dependent_forest_10 | 16,000 / 100 | 257.967 → 257.425 | -0.18% | +0.82% | -0.39% |
| rbf_pairs_windowed | 16,384 / 16,384 | 318.490 → 323.554 | -0.33% | -0.02% | -0.48% |
| secp256k1 | 4,096 / 32 | 559.926 → 553.826 | -1.08% | -0.90% | -0.28% |

| Workload | Transactions/s, remote → final | CPU µs/target, remote → final | Peak RSS MiB, remote → final | p99 ms, remote → final |
|---|---:|---:|---:|---:|
| always_success | 64,504.5 → 64,694.3 | 155.60 → 155.97 | 204.31 → 205.25 | 243.153 → 242.623 |
| dependent_forest_10 | 62,023.3 → 62,154.0 | 159.61 → 159.23 | 207.28 → 206.55 | 253.514 → 253.004 |
| rbf_pairs_windowed | 51,442.7 → 50,637.6 | 182.38 → 182.45 | 238.84 → 237.80 | 314.428 → 317.335 |
| secp256k1 | 7,315.3 → 7,395.8 | 1135.64 → 1123.79 | 164.28 → 163.58 | 552.558 → 548.001 |

Absolute values are medians; changes are medians of paired ratios. Lower is
better for these cost metrics. This is a three-pair screen, with no confidence
interval or equivalence claim. 4 always-success target windows remain below
the existing 0.25-second timing floor; they remain short observations rather
than formal passes. All attempts are retained.

### Transaction selection

A preceding seven-case screen found a material packing slowdown, triggering the
complete 20-case matrix: five graph shapes, equal/CPFP fees and All/Partial limits,
16,384 entries, ancestor limit 64. Partial permits 595,000 bytes and
3,500,000,000 cycles. Chains are bounded cohorts. Each case has 12 balanced
independent process pairs, 64 measured selections per process and three warmups:
480 runs. Each pair produces identical transaction sets, order, fees, bytes and
cycles. No failed attempts are substituted.

The statistic is each fresh process's mean selection time. Qualification requires
at least eight calls, 20 ms accumulated selection time and clock-anchor brackets
at most 0.1% of that time. Every original per-call 1 ms qualification is retained
separately. Intervals are conservative pointwise 95% sign/order-statistic
intervals for paired median ratios, assuming stable independent sampling;
they do not provide simultaneous coverage across the matrix. Direct A/B was
used without adding an A/A gate.

| Shape / fees / limit | Remote ms | Final ms | Final/remote ratio [95% interval] |
|---|---:|---:|---:|
| independent / equal / All | 3.467 | 4.613 | 1.3439 [1.2782, 1.3699] |
| independent / equal / Partial | 1.369 | 3.522 | 2.5567 [2.4420, 2.6847] |
| independent / cpfp / All | 3.350 | 4.640 | 1.3978 [1.2998, 1.4332] |
| independent / cpfp / Partial | 1.345 | 3.347 | 2.4826 [2.4376, 2.5168] |
| chain / equal / All | 3.345 | 4.898 | 1.4831 [1.4210, 1.5500] |
| chain / equal / Partial | 2.014 | 4.133 | 2.0428 [1.9255, 2.1922] |
| chain / cpfp / All | 3.510 | 5.011 | 1.3983 [1.3665, 1.5861] |
| chain / cpfp / Partial | 2.017 | 4.358 | 2.1128 [2.0330, 2.2504] |
| fanout / equal / All | 5.679 | 7.114 | 1.2009 [1.1768, 1.2976] |
| fanout / equal / Partial | 2.993 | 5.226 | 1.7829 [1.6900, 1.8674] |
| fanout / cpfp / All | 5.036 | 6.800 | 1.3052 [1.2706, 1.3582] |
| fanout / cpfp / Partial | 2.146 | 4.308 | 1.9514 [1.8327, 2.1148] |
| diamond / equal / All | 4.704 | 5.972 | 1.2670 [1.2368, 1.3221] |
| diamond / equal / Partial | 2.492 | 5.146 | 2.0304 [1.9763, 2.0943] |
| diamond / cpfp / All | 4.438 | 5.654 | 1.3051 [1.2200, 1.3326] |
| diamond / cpfp / Partial | 2.563 | 5.195 | 2.0418 [1.9390, 2.1725] |
| mixed / equal / All | 5.509 | 6.255 | 1.1682 [1.1150, 1.2694] |
| mixed / equal / Partial | 2.951 | 6.267 | 2.0477 [1.9519, 2.2391] |
| mixed / cpfp / All | 4.920 | 6.256 | 1.2819 [1.2665, 1.3444] |
| mixed / cpfp / Partial | 3.248 | 6.454 | 2.1289 [1.9993, 2.2003] |

There are 0 lower-time, 20 higher-time and
0 unresolved cases by those intervals. These are selection
costs, including graph construction and destruction on every call. The adapter
does not exercise template graph-cache hits. VM verification, admission, DAO,
Store capture/locks and full block serialization are outside this window.
The matrix does not establish whole-node CPU/RSS or mining throughput.

## Design costs and interpretation

Current admission closes a dep-cell's reader set when its spender is accepted.
All earlier admitted readers must precede spending, even across blocks; later
readers are rejected. Causal ancestry and read-before-spend prerequisites remain
separate. The tracking baseline ordered only readers already selected for the
same block. Restoring that shortcut would violate the current contract.

The common benchmark corpus does not spend a dep cell read by another fixture
entry, so the measured outputs are comparable. Complete prerequisite processing
still adds work. The final refinement removes the full priority-to-ordinal sort
and orders ready nodes directly by the existing package key; it introduces no
additional persistent cache. The measurements compare complete versions and do
not isolate that refinement's contribution.

The wider design trades machinery for explicit guarantees: immutable owners,
original-read validation, coupled commits, bounded work and ordered publication.
Independent chain controls remove the Suspend/IBD/handler wait cycle. Local RPC
uses unbudgeted canonical verification; network VM budgets use machine-local
calibration. These correctness and liveness properties have behavioral tests;
steady-state throughput does not prove them.

## Historical develop evidence

The earlier migration study used candidate `604edc2231b68b0e79a949f511112c1e1fa02c49`
against develop production `95fd03933ce2a396b84aed12caaa0f984165cd6c`, prepared as
`6f63f5992cef9b6f6496cf4ce2c04aca0f028d7f` with the same measurement bundle.
Seven receive-to-terminal workloads qualified: eight-worker always-success and
dependent-forest throughput improved by 146% and 199%; secp improved by 0.48%.
All primary lifetime RSS comparisons increased, and several cheap-script
concurrent workloads used more CPU. Reorg failed its CPU precision gate and
has no qualified overall performance ranking.

A separate packing study used `8aec0d70bc0e86f6bb86e6dfa4aac07e658c662f` against
the same develop production basis. Its 20-case direct comparison had 13
lower-time and seven higher-time cases. Partial results could differ under the
legacy tie policy, so it was a delivered-policy comparison rather than a universal
equal-work speedup. Later runtime changes do not inherit either study's ratios.

## Reproduction and retained evidence

Use the maintained [capture commands](BENCHMARK.md#measure-template-transaction-selection)
and [comparison workflow](BENCHMARK.md#prepare-a-comparison). Freeze source,
toolchain, features, workload, binary hashes and decision rules before running;
retain raw attempts and all qualification failures. Rebuild workspace artifacts
when switching sources through a shared Cargo target directory.

The latest delivery records are `perf-final-results.json`,
`perf-packing-final-plan.json`, `perf-admission-final-plan.json`,
`perf-final-attempts/`, `perf-final-builds.json` and `perf-light-builds.json`.
The complete report and original 731-line historical narrative are retained in
the project delivery records; the repository keeps this source-bound summary.
Earlier portable packets retain their original plans, failures, binaries and
offline replay: `tx-pool-g0-20260911-evidence.tar.gz`,
`tx-pool-packing-20260911-evidence.tar.gz`,
`tx-pool-stability-sequences-20260912-evidence.tar.gz` and
`tx-pool-rejection-refinement-20260912-evidence.tar.gz`.
Replaying measurements verifies recorded evidence; it does not recreate OS
scheduling or establish performance on other machines.
