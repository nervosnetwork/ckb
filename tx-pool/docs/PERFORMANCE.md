# Transaction-pool performance evidence

Performance belongs to the measured executable inputs. The latest local study
uses current `8b1cf0a7c20e60c11f98647c8577bdfb0481e8ea`, pre-refinement
`670a58042824ea0665a86abab8f8535a2b1d6417`, and historical PR head
`54c26b2e1fe2954ade628304f471cc81e73b6f6b`. Later documentation-only commits do
not change these executables. Earlier develop studies use different sources.

[Architecture](ARCHITECTURE.md) describes the design;
[benchmarking](BENCHMARK.md) defines captures and qualification;
[profiling](PROFILING.md) covers attribution; and
[maintenance](MAINTENANCE.md) records operational contracts.

## Current cold selection

Measured on macOS 26.6.2 arm64, 18 logical CPUs, Rust 1.95.0, profile `prod`,
CKB-VM 0.24.15 with matching `detect-asm` features. Profiling and allocation
instrumentation were disabled. The three arms use identical Rust packing harness
sources. Relevant node regressions passed before measurement; clean sources and
frozen binary hashes were checked before and after capture.

The predefined matrix has five graph shapes, equal/CPFP fees and All/Partial
limits: 16,384 entries, ancestor limit 64, bounded chain cohorts. Partial permits
595,000 bytes and 3,500,000,000 cycles. Three Latin-order blocks compare all three
versions, giving 180 fresh-process captures. Each process performs 32 measured
selections after three warmups. The external three-arm capture driver compares
each capture's fixture digest, complete result receipt and byte/cycle limits with
the other arms in its block. Those receipts agree on transaction set, order, fees,
bytes and cycles. The Rust harness alone checks within-arm determinism.
No failed samples were substituted.

Absolute values below are medians of process means; changes are medians of
paired ratios. Every process meets the accumulated-time qualification: at least
eight calls, 20 ms selection time and clock-anchor uncertainty at most 0.1% of
that time. Original per-call qualification is retained separately. Three pairs
provide a diagnostic screen, without confidence intervals or an equivalence claim.

| Shape / fees / limit | Remote ms | Before ms | Current ms | Current vs before | Current vs remote |
|---|---:|---:|---:|---:|---:|
| independent / equal / All | 3.646 | 5.067 | 4.218 | -16.8% | +13.8% |
| independent / equal / Partial | 1.389 | 3.875 | 2.970 | -23.4% | +114.2% |
| independent / cpfp / All | 3.574 | 4.975 | 4.200 | -15.6% | +17.0% |
| independent / cpfp / Partial | 1.378 | 3.564 | 2.988 | -16.0% | +116.9% |
| chain / equal / All | 3.895 | 5.695 | 4.513 | -20.9% | +16.2% |
| chain / equal / Partial | 2.289 | 5.078 | 3.866 | -23.4% | +68.9% |
| chain / cpfp / All | 3.628 | 5.108 | 4.482 | -12.3% | +20.3% |
| chain / cpfp / Partial | 2.244 | 4.569 | 3.937 | -13.8% | +73.4% |
| fanout / equal / All | 6.078 | 7.922 | 7.067 | -10.5% | +16.7% |
| fanout / equal / Partial | 3.103 | 5.363 | 4.767 | -14.8% | +49.2% |
| fanout / cpfp / All | 5.736 | 7.683 | 6.533 | -13.5% | +13.9% |
| fanout / cpfp / Partial | 2.425 | 4.551 | 3.925 | -13.7% | +66.4% |
| diamond / equal / All | 5.097 | 6.170 | 5.463 | -11.0% | +7.2% |
| diamond / equal / Partial | 2.759 | 6.051 | 4.793 | -20.8% | +77.6% |
| diamond / cpfp / All | 5.040 | 6.181 | 5.658 | -5.3% | +12.3% |
| diamond / cpfp / Partial | 2.757 | 5.826 | 5.057 | -11.0% | +90.5% |
| mixed / equal / All | 5.718 | 6.512 | 5.972 | -9.8% | +5.8% |
| mixed / equal / Partial | 3.177 | 6.624 | 6.240 | -5.8% | +92.9% |
| mixed / cpfp / All | 5.380 | 6.778 | 5.901 | -9.0% | +7.6% |
| mixed / cpfp / Partial | 3.263 | 7.300 | 6.031 | -17.4% | +87.2% |

The refinement lowers paired median time by 5.3%–23.4%, but current selection
remains 5.8%–116.9% slower than the historical PR head. These windows include graph
construction and destruction on every call. They exclude VM verification,
admission, Store capture/locks, DAO and block serialization, and do not exercise
template-cache hits. They do not establish mining throughput or whole-node RSS.

### Attribution and allocation

A pre-refinement macOS sampling trace for independent/equal/Partial placed
244 of 365 selection-stack samples in prerequisite processing, including map
growth, hashing and priority comparisons. Sampling identifies where to inspect;
it is not an exact phase-time decomposition.

The refinement constructs reversed adjacency directly in compact arrays, shared
by precedence and strongly connected component traversal. It removes temporary
edge tuples, per-vertex reverse containers and redundant sorting, and pre-sizes
the spender map. Selection priority and accepted-reader obligations are unchanged.

Separate instrumented builds compare current and pre-refinement allocation
traffic on three Partial/equal shapes. Each process has three measured selections
and one warmup; corpus and result checks match. These are process-wide alloc and
realloc requests during selection, including any concurrent allocations. They
measure neither retained bytes nor RSS, and their timings are excluded above.

| Shape | Calls, before → current | Change | Requested MiB, before → current | Change |
|---|---:|---:|---:|---:|
| independent | 97 → 84 | -13.4% | 10.753 → 9.972 | -7.3% |
| chain | 120 → 94 | -21.7% | 10.277 → 8.996 | -12.5% |
| mixed | 128 → 101 | -21.1% | 12.396 → 10.614 | -14.4% |

## Actual template RPCs

The current prod node's `TemplatesDuringAdmission` regression starts with 256
proposed independent transactions. It checks 64 warm RPCs, removes one selected
owner, admits another 256 transactions concurrently, requires their exact proposal
set to become visible, and successfully submits the resulting block.

| Observation | Requests | Median ms | p95 ms |
|---|---:|---:|---:|
| Warm template reuse | 64 | 1.293 | 1.402 |
| While admission progresses | 25 | 1.455 | 1.547 |
| Immediately after selected-owner removal | 1 | 1.686 | — |

All 25 admission-phase calls observed submission completions between their start
and end. The 256 admissions completed in 37.223 ms; their proposals became visible
109.480 ms after admission began. Selected-owner removal immediately produced a
valid updated commit set. This is one local run, including RPC queueing and
transport. It does not replace deterministic saturated-handler liveness tests,
or establish latency under every load. Request, result, work ID and admission
progress observations are retained for each call.

## Network admission and calibration

The earlier 24-run admission screen compares `670a58042` with `54c26b2e`:
always-success, dependent forest, RBF pairs and real secp256k1, eight workers and
four peers. Paired median elapsed changes range from -1.08% to -0.06%; four cheap
script windows are shorter than the 0.25-second floor. No equivalence or material
throughput improvement follows from that screen. These paths were unchanged by
the compact-graph refinement; the current real-secp check completed all 272
transactions, including 16 warmups, with no duplicate or rejected relay terminal.

Local calibration observations use the node's actual AsmMachine backend. Three
fresh processes yielded 162,035–243,838 cycles/ms and 22–32 ms internal minima.
All 54 independent branch/division holdouts, spanning V0/V1/V2, 4 KiB/1 MiB loads
and 1M/50M cycles, fit their budgets; the lowest observed budget/time ratio was
43.8. These fixed workloads do not bound arbitrary ELF parsing, provider I/O or
host contention. The [cross-machine workflow](BENCHMARK.md#cross-machine-diagnostics)
records runner identities and retains failed observations; its results must be
assessed independently of these local measurements.

## Design costs and historical evidence

Admission closes a dep-cell's reader set when its spender is accepted. All earlier
admitted readers must precede spending, even across blocks; later readers are
rejected. Causal ancestry and read-before-spend prerequisites remain separate.
The historical PR head ordered only readers already selected for the same block.
Restoring that shortcut would violate the current contract. The common corpus
has no dep-cell spender, so outputs remain comparable while the current complete
prerequisite processing still costs time. No persistent cache was added to conceal
cold-selection work.

The wider design pays for immutable owners, original-read validation, coupled
commits, bounded work and ordered publication. Independent chain controls remove
the Suspend/IBD/handler wait cycle. Local RPC uses unbudgeted canonical verification;
only network work uses calibrated VM budgets. Behavioral tests, rather than
steady-state throughput, establish those contracts.

The older develop study used candidate `604edc2231b68b0e79a949f511112c1e1fa02c49`
against production `95fd03933ce2a396b84aed12caaa0f984165cd6c` with an adapter.
Eight-worker always-success and dependent-forest throughput improved 146% and
199%; secp improved 0.48%. All primary lifetime RSS comparisons increased, and
several cheap-script workloads used more CPU. Reorg lacked CPU precision.
An earlier packing comparison had 13 lower-time and seven higher-time cases,
with some different legacy selections. Current sources do not inherit those ratios.

## Reproduction and evidence

Use the maintained [capture commands](BENCHMARK.md#measure-template-transaction-selection)
and [comparison workflow](BENCHMARK.md#prepare-a-comparison). Freeze source,
toolchain, features, workload, binary hashes and decision rules before running;
retain attempts and qualification failures. Rebuild affected workspace artifacts
when switching sources through a shared Cargo target directory.

The latest project delivery records are under `four-part-validation/`:
`packing-plan.json`, `packing-screen/`, `packing-screen-results.json`,
`final-build-identity.json`, `allocation-build-receipts.json`, `allocation/`,
`allocation-summary.json`, `native-final-stdout.log`, `template-final-summary.json`
and `calibration-holdouts-gated.log`. Earlier full-matrix intervals and admission
results remain in `perf-final-results.json` and their original capture directories.
Raw packets retain earlier plans, failures and offline replay. Replaying evidence
does not recreate OS scheduling or establish performance on another machine.
