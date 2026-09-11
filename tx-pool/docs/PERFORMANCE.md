# Transaction-pool performance evidence

The final frozen candidate improves all seven qualified receive-to-terminal
workloads against the prepared develop basis. Eight-worker secp throughput improves
by 0.48%; always-success and dependent-forest throughput improve by 146% and 199%.
Those gains have costs: all observed primary lifetime RSS comparisons increase,
and several concurrent cheap-script workloads use more process CPU. Reorg completes
its functional contract but fails the CPU precision gate, so it has no qualified
overall performance ranking.

This evidence supports the complete migration's correctness, capacity and concurrent
throughput benefits. Secp throughput alone would not justify its API and maintenance
cost. The conclusion is a finite engineering assessment on this host, not a claim
of universally better resource efficiency or an independent maintainer approval.

[Architecture](ARCHITECTURE.md) explains the mechanisms and ownership boundaries.
[Benchmarking](BENCHMARK.md) defines workloads and qualification;
[profiling](PROFILING.md) separates causal diagnostics from acceptance timing.
[Maintenance](MAINTENANCE.md) describes migration and operating costs.

## Source and measurement contract

Candidate `604edc2231b68b0e79a949f511112c1e1fa02c49` is the immutable executable
source snapshot. Develop production basis is
`95fd03933ce2a396b84aed12caaa0f984165cd6c`; prepared commit
`6f63f5992cef9b6f6496cf4ce2c04aca0f028d7f` adds the same measurement bundle and
legacy adapter without changing develop production Rust. Both use locked CKB-VM
0.24.15 with matching enabled VM features; there is no VM fork, patch or vendoring.
Results were collected on macOS 26.6.2 arm64, 18 logical CPUs, Rust 1.95.0, profile
`prod`. Profiling, Console and allocation observation were disabled. Four simulated
peers were used throughout. Reporting edits after measurement do not change
executable inputs; the evidence records their separate hashes.

Each section applies to its named source; the packing comparison identifies its
own earlier revisions. Later executable changes require separate measurements
and do not inherit the ratios or verification counts reported here.

Each row has 24 paired samples of four fresh-process replicates per side, plus
two retained pilots. Pair order is balanced and prospectively randomized. The
unchanged gates require every target window ≥0.25 s, throughput paired relative
MAD ≤1.5%, and each primary 95% ratio interval width ≤4%. A/A additionally requires
all three complete primary intervals within [0.98, 1.02]. All eight A/A rows pass.
Seven A/B rows pass all three primary gates; reorg remains `imprecise`, and the A/B
runner correctly exits 2. No failed or imprecise result was replaced.

The frozen ascending calibration ladder retained 14 populations/28 pilots,
including six short populations, before fixing the eight formal populations.
Each complete A/A and A/B study retains 1,552 successful executions. Together with
two original fanout stress executions and the 294 secp attribution executions,
the final end-to-end/secp studies contain 3,428 native executions. Successful executions do not
turn an imprecise row into a pass.

Target elapsed time uses one monotonic clock from submission start to required
callback/relay completion; fixture construction and post-window set validation are
excluded. CPU is process user+system time over that window. RSS is each process's
lifetime peak, including setup, warmup and shutdown; the primary sample uses the
mean of all four peaks, and their maximum remains diagnostic. CPU is not wall time,
allocation traffic is not retained memory, and pool budgets are not an RSS limit.

Earlier phase probes localized RSS spread mainly to target/reorg work after a
stable fixture/service setup. They did not isolate allocator retention, transient
queue occupancy or runtime scheduling as a unique cause. The former maximum of
four peaks estimated an extreme; the prospectively declared mean estimates the
average replicate lifetime peak and retains the extreme as a diagnostic. This is
an explicit change of measurement question, not proof that OS memory variability
disappeared. The old failed A/A remains failed; the new A/A checks the new question
under its declared unchanged precision and equivalence margins.

Bracketed wall anchors independently qualify profiler alignment. Two A/A and five
A/B attempts fail that diagnostic and remain unsuitable for aligned profiling;
their monotonic timing and terminal contracts pass. A disagreement between
separately sampled clocks no longer corrupts the target duration or rejects it.
This prospective protocol repair does not requalify old failed attempts.

## Final develop comparison

Absolute values below are medians over the 24 replicate aggregates, shown as
develop → candidate. CPU is normalized by target transactions (four times the
listed target per aggregate); RSS is MiB. Ratios below are medians of paired ratios,
so they need not equal the ratio of these separate medians.

| Workload / workers | Target / warm per process | Transactions/s | CPU µs/target tx | Mean lifetime peak RSS MiB |
|---|---:|---:|---:|---:|
| `always_success / 1` | 16,000 / 1,000 | 11,223.4 → 16,414.6 | 124.50 → 93.94 | 176.48 → 207.12 |
| `always_success / 8` | 32,000 / 1,000 | 26,013.9 → 64,160.3 | 136.12 → 157.93 | 248.77 → 290.16 |
| `secp256k1 / 1` | 4,000 / 100 | 1,158.3 → 1,171.2 | 899.36 → 888.53 | 122.35 → 136.26 |
| `secp256k1 / 8` | 4,000 / 100 | 7,973.2 → 8,015.6 | 1,041.74 → 1,038.97 | 149.90 → 163.59 |
| `dependent_forest_10 / 8` | 32,000 / 1,000 | 20,948.3 → 62,665.6 | 142.80 → 159.15 | 227.66 → 293.41 |
| `fanout_ready_64_reverse / 8` | 8,320 / 0 | 11,593.9 → 20,383.1 | 128.51 → 195.53 | 115.36 → 145.40 |
| `rbf_pairs / 8` | 32,768 / 32,768 | 44,853.6 → 50,538.8 | 116.63 → 187.65 | 298.20 → 365.13 |
| `reorg_in_flight / 8 †` | 2,000 / 100 | 1,252.3 → 1,304.9 | 290.67 → 171.29 | 111.88 → 126.32 |

Higher throughput is better; lower CPU/RSS is better. Brackets are conservative
95% pointwise median intervals, conditional on stable independent sampling.
Balanced order and low MAD do not prove sampling independence or simultaneous
coverage across the matrix.

| Workload / workers | Throughput ratio [interval] | CPU ratio [interval] | RSS ratio [interval] |
|---|---|---|---|
| `always_success / 1` | 1.4618 [1.4591, 1.4657] | 0.7542 [0.7530, 0.7567] | 1.1715 [1.1691, 1.1773] |
| `always_success / 8` | 2.4642 [2.4587, 2.4742] | 1.1599 [1.1585, 1.1616] | 1.1660 [1.1617, 1.1708] |
| `secp256k1 / 1` | 1.0111 [1.0102, 1.0125] | 0.9881 [0.9868, 0.9888] | 1.1140 [1.1117, 1.1165] |
| `secp256k1 / 8` | 1.0048 [1.0037, 1.0075] | 0.9975 [0.9951, 0.9988] | 1.0906 [1.0897, 1.0942] |
| `dependent_forest_10 / 8` | 2.9923 [2.9812, 3.0035] | 1.1145 [1.1110, 1.1180] | 1.2897 [1.2837, 1.2928] |
| `fanout_ready_64_reverse / 8` | 1.7539 [1.7504, 1.7701] | 1.5236 [1.5150, 1.5256] | 1.2594 [1.2562, 1.2650] |
| `rbf_pairs / 8` | 1.1284 [1.1234, 1.1326] | 1.6097 [1.6043, 1.6125] | 1.2239 [1.2156, 1.2347] |
| `reorg_in_flight / 8 †` | 1.0448 [1.0431, 1.0462] | 0.5878 [0.5721, 0.6065] | 1.1289 [1.1206, 1.1371] |

† Reorg values are descriptive. Its CPU interval width is 5.8592%, exceeding the
unchanged 4% gate; the entire row is excluded from overall performance ranking.
Across its 96 formal executions per side, develop CPU ranges from 449.930 to
878.493 ms (median 585.416); candidate ranges from 331.924 to 352.975 ms (median
341.779). This establishes greater observed develop CPU dispersion, not an
isolated scheduler or lock cause. Develop has no duplicate callbacks; candidate
has permitted reacceptance callbacks with weak CPU/duplicate correlation (0.128).
Duplicate counts therefore do not explain the develop dispersion.

The reorg fixture observes one callback in flight before requesting the tip update,
with an artificial 500 µs callback delay. That event proves overlap, not a fixed
amount of pending computation. Both implementations complete the same corpus and
required terminal contract. The window is not block acceptance or completed
reconciliation; API-return and stop observations also cannot rank joined node
shutdown. The raw data and source-bound dispersion analysis retain this limitation.

Ready reverse fanout submits independent 64-child cohorts below the legacy orphan
capacity, with public orphan-count barriers. Their query and waiting costs are
inside the target window: median barrier fractions are 0.75% for develop and 4.24%
for candidate. This is a submission/recovery/barrier workload, not isolated graph
processing. Its original 5,752-transaction single-parent stress remains separate:
candidate accepts all 5,752; develop accepts 101 and explicitly rejects 5,651. Both
settle complete disjoint terminal sets with zero unresolved entries, duplicates or
generation resets. Unequal accepted populations permit a capacity/recovery result,
not a throughput ratio.

RBF performs actual victim replacement. Candidate also settles one mandatory
victim rejection notice for each of the 32,768 warm transactions, whereas develop
does not expose those notices. Its 12.84% throughput improvement and 60.97% higher
CPU are the complete delivered behavior, not a pure equal-effect efficiency test.
More broadly, concurrent cheap-script gains trade increased parallel computation
and publication work for shorter completion time. The measurements do not isolate
how much each mechanism contributes to that CPU cost.

## Controlled secp attribution

The historical slowdown is retained and independently attributed with six arms:
A is old `190a8140` with its original harness; B uses that production with the prior
`f9c6843c` harness; C is prior `f9c6843c` with its own harness; D uses prior production
with the final harness; E is final `604edc22`; F is the byte-identical final control.
All historical arms were rebuilt serially with the final Rust toolchain and profile,
invalidating workspace package artifacts. All use 4,000 targets, 100 warm, eight
workers and four peers with identical transaction/cycle corpora.

The prospective six-arm design has six pilots plus 24 blocks × two replicates ×
six arms. Positions and directed predecessor pairs are balanced. Original narrow
throughput gates remain: individual duration ≥0.25 s, MAD ≤1.5%, interval width
≤0.5%, and same-binary control wholly within ±0.25%. All contrast quality gates and
the same-binary equivalence gate pass. A direction still requires its interval to
exclude 1. CPU/RSS are described separately from this narrow throughput gate.

| Controlled contrast | Throughput ratio [95% interval] | Disposition |
|---|---|---|
| Prior delivered / old delivered (C/A) | 0.984333 [0.983149, 0.986383] | Lower |
| Prior / old production, fixed prior harness (C/B) | 0.988091 [0.986474, 0.989867] | Lower |
| Prior / original harness, fixed old production (B/A) | 0.997327 [0.995981, 0.998060] | Lower |
| Current / prior harness, fixed prior production (D/C) | 0.998865 [0.997220, 1.001498] | Unresolved |
| Final / prior production, fixed current harness (E/D) | 1.020627 [1.019341, 1.022652] | Higher |
| Final delivered / old delivered (E/A) | 1.006080 [1.003038, 1.007734] | Higher |
| Byte-identical final control (E/F) | 0.999778 [0.997936, 1.000972] | Equivalent within ±0.25% |

At fixed final harness, final production improves secp throughput 2.06% over the
prior candidate, with CPU ratio 0.98006 [0.97795, 0.98048] and mean peak RSS ratio
0.98583 [0.98432, 0.98789]. The former delivered regression contains both production
and harness contributions. The prior harness delta changes callback/relay atomic
observation orderings; allocation-counter changes in that patch are disabled in
these timing builds. The controlled whole-harness effect does not isolate one
atomic operation. The latest harness effect on prior production is unresolved.

Historical v2 outputs retain their schema: native `BENCH_RESULT` supplies the
original monotonic Instant duration, while unbracketed wall endpoints cannot
qualify aligned profiling. They are never rewritten as v3 or used to recover an
old failed-study pass. The former cross-batch 0.271% observation is not the new
controlled harness estimate. Production contrasts include non-VM dependency changes
and do not isolate one pool optimization. Paired median contrasts also need not
multiply exactly into the total delivered ratio.

## Packing comparison

Packing was measured separately from receive-to-terminal throughput. Its current
candidate performs more work per selection than develop in most of the focused
large-fixture cases below. The exception is equal-fee fanout. The dense aggregate
optimization reduces this cost relative to the previous candidate, but does not
establish a generally faster selector than develop.

These are earlier, separately frozen `packing-r28` sources: candidate
`74926165483b857cf502b47e8c9690939a616676`, control
`894c729e4864f0f94b993cc2a2db842f38ecc34b`, and prepared develop
`c4cc26b261e53b2326f24f90a0c3d623df2d87a2` based on
`cdfde29e45dfbe9443be66083241ebc1acca9fe6`. They are not the final executable
pair in the receive-to-terminal matrix. The candidate/control production change
is bounded ordinal scratch for full-graph aggregates. All three arms invoke their
actual production selector through the shared v2 harness and their respective
adapters; the legacy partial-set qualification failure from `packing-r26` remains
preserved and was not reused as passing evidence.

The [selection window](BENCHMARK.md#measure-template-transaction-selection) includes current
owner-vector cloning, derived selection-state construction, packing and destruction
of that state. Develop selects from its already maintained `PoolMap`; the cost of
maintaining that map during admission is outside this window. Fixture/source
preparation, Store capture/locks, optional content and final DAO/template building
are excluded. These elapsed times cannot be read as complete block-template latency
or receive throughput. No packing process-CPU or RSS comparison was established.

### Develop versus candidate: complete large-fixture matrix

Each cell is develop → candidate, in milliseconds per selection, with lower being
better. The values are medians of five calls after two warm calls in one process
per arm/case. Arm order was fixed; these are descriptive observations without an
independent-process develop A/A qualification or confidence interval. All 40 large
cases are shown, including adverse results. `All` fits the whole fixture; `Partial`
limits selection to 595,000 bytes and 3,500,000,000 cycles. `chain` is a forest of
bounded chains, not one 16,384-ancestor transaction chain.

| Shape / fee pattern | Pool entries | All: develop → candidate ms | Partial: develop → candidate ms |
|---|---:|---:|---:|
| `independent / equal` | 4,096 | 0.958 → 2.501 | 0.540 → 1.918 |
| `independent / equal` | 16,384 | 4.273 → 15.858 | 0.612 → 8.437 |
| `independent / cpfp` | 4,096 | 0.948 → 2.466 | 0.536 → 1.963 |
| `independent / cpfp` | 16,384 | 4.482 → 16.637 | 0.612 → 8.422 |
| `chain / equal` | 4,096 | 11.547 → 12.374 | 5.596 → 10.536 |
| `chain / equal` | 16,384 | 49.842 → 92.256 | 5.812 → 74.624 |
| `chain / cpfp` | 4,096 | 11.612 → 13.008 | 5.547 → 10.803 |
| `chain / cpfp` | 16,384 | 49.771 → 93.339 | 5.704 → 69.261 |
| `fanout / equal` | 4,096 | 17.445 → 4.467 | 16.167 → 3.289 |
| `fanout / equal` | 16,384 | 256.194 → 31.644 | 44.028 → 19.147 |
| `fanout / cpfp` | 4,096 | 2.312 → 4.256 | 0.954 → 3.038 |
| `fanout / cpfp` | 16,384 | 11.047 → 31.384 | 1.081 → 17.893 |
| `diamond / equal` | 4,096 | 1.353 → 3.659 | 0.793 → 3.407 |
| `diamond / equal` | 16,384 | 6.851 → 28.010 | 0.797 → 17.918 |
| `diamond / cpfp` | 4,096 | 1.376 → 3.849 | 0.722 → 2.978 |
| `diamond / cpfp` | 16,384 | 6.830 → 28.111 | 0.838 → 18.554 |
| `mixed / equal` | 4,096 | 1.983 → 4.527 | 0.994 → 3.404 |
| `mixed / equal` | 16,384 | 10.941 → 32.722 | 1.140 → 21.986 |
| `mixed / cpfp` | 4,096 | 2.038 → 4.289 | 1.002 → 3.343 |
| `mixed / cpfp` | 16,384 | 10.190 → 33.411 | 1.088 → 20.204 |

Candidate selection takes longer than develop in 36/40 large cases. In the
16,384-entry partial cases, observed candidate/develop elapsed ratios range from
0.435 for equal-fee fanout to 22.495 for equal-fee diamond. These ratios describe
the recorded runs, not qualified final-source speedups or slowdowns.

Selection quality is reported alongside cost. Candidate and control produce the
same selected result in every case. All-fit runs select all entries. Across the
40 large cases, develop and candidate have equal selected fee totals in 39 cases;
partial 16,384-entry CPFP fanout selects 1,453 → 1,473 transactions and
44,300,517,760 → 44,951,454,520 shannons (1.47% more fees), while selection takes
1.081 → 17.893 ms. Partial-result sets and cycle utilization can differ even where
fees match. This is neither a proof of globally optimal packing nor an equal-work
comparison for that differing-result case. The 15 eight-entry functional cases
and their exhaustive small-fixture checks remain in the same source packet.

### Allocation traffic

These separate instrumented runs use 4,096 entries, CPFP fees, three measured
calls after one warm call. Values are requested allocation bytes per selection,
develop → candidate. They count allocation traffic, not retained memory or RSS;
instrumented elapsed times are excluded from the timing table.

| Shape | All: requested bytes | Partial: requested bytes |
|---|---:|---:|
| `independent` | 5,447,300 → 7,275,808 | 2,639,282 → 6,789,648 |
| `chain` | 17,795,260 → 9,187,080 | 8,441,018 → 8,544,424 |
| `fanout` | 7,069,454 → 9,605,216 | 2,690,140 → 8,471,760 |
| `diamond` | 4,081,428 → 8,860,792 | 2,000,690 → 8,193,576 |
| `mixed` | 4,662,652 → 9,037,288 | 2,094,948 → 8,222,360 |

### Candidate optimization: prospective A/A and A/B

`packing-r29` reuses the frozen control/candidate binaries above. It compares the
candidate optimization against its previous implementation, **not against develop**.
Ten 16,384-entry CPFP cases use six balanced paired blocks for each of A/A and A/B,
with one fresh process per side and five calls after two warm calls per process:
240 captures and replays in total. The sampling unit is the process median.

The original A/A gate requires its complete pointwise interval inside
[1/1.05, 1.05]; only independent/partial passes. A/B establishes lower elapsed time
only when A/A passes and the A/B interval lies below 1. Brackets below are the
original conservative order-statistic intervals, conditional on independent
blocks, without simultaneous coverage across cases. All unresolved rows remain.

| Shape / budget | A/A elapsed ratio [interval] | Candidate/control elapsed ratio [interval] | Original decision |
|---|---|---|---|
| `independent / All` | 0.9792 [0.9332, 1.0571] | 0.8203 [0.7799, 0.8833] | Unresolved |
| `independent / Partial` | 0.9932 [0.9647, 1.0237] | 0.6993 [0.6665, 0.7532] | Lower selection time |
| `chain / All` | 0.9912 [0.8703, 1.0218] | 0.3880 [0.3768, 0.4373] | Unresolved |
| `chain / Partial` | 0.9896 [0.9461, 1.0088] | 0.3423 [0.3355, 0.3471] | Unresolved |
| `fanout / All` | 0.9779 [0.8537, 1.0333] | 0.8244 [0.7700, 0.8526] | Unresolved |
| `fanout / Partial` | 1.0104 [0.9833, 1.0697] | 0.7155 [0.6560, 0.7590] | Unresolved |
| `diamond / All` | 0.9902 [0.9133, 1.0458] | 0.7759 [0.7457, 0.9206] | Unresolved |
| `diamond / Partial` | 1.0418 [0.7167, 1.0533] | 0.6717 [0.6203, 0.7362] | Unresolved |
| `mixed / All` | 0.9749 [0.9444, 1.0271] | 0.7135 [0.6868, 0.7723] | Unresolved |
| `mixed / Partial` | 0.9849 [0.9244, 1.0454] | 0.6193 [0.6102, 0.6271] | Unresolved |

The qualified independent/partial optimization has elapsed ratio 0.6993
[0.6665, 0.7532], about 30.1% lower than the prior candidate. The other nine A/B
point estimates are favorable but remain unresolved under their original A/A
gates. None supplies a qualified final-candidate/develop packing ranking.

All 195 exploratory captures/replays and all 240 prospective captures/replays are
retained under `evidence/candidate-reassessment-20260910-r1/` in the portable
packet. The source/binary manifests, `packing-r28/summary.json`,
`packing-r28/observations.json`, and `packing-r29/plan.json`/`result.json` identify
the exact inputs, complete results and qualification. These focused runs are
separate from the 3,428 final end-to-end/secp executions.

## Resource and maintenance decision

The retained improvements reduce repeated owner/index traversal, active-account
construction, graph calculation, root-program parsing and syscall data reads.
Temporary dense ordinals/marks replace repeated graph bookkeeping; they do not
add a persistent membership cache. Bounded program metadata reuse retains its
thread-local cost and excludes VM execution state. The exact validity, invalidation
and ownership rules are in [commit](architecture/COMMIT.md#current-optimizations)
and [execution](architecture/EXECUTION.md#current-optimizations).

Focused packets preserve allocation and adverse results independently of the
final timing matrix. Earlier source-bound secp sampling exposed VM execution,
VM setup and publication/scheduling work, which selected the bounded metadata,
data-read and singleton-publication changes for controlled investigation. A
parked stack is not CPU time spent waiting, overlapping spans are not additive,
and those instrumented weights do not prove a lock-contention bottleneck. The
final controlled production comparison measures the combined result.

For example, a real-snapshot DAO small-churn fixture with
16,584 live cells improves median memo work from 15.022 to 3.631 ms and requested
allocation traffic from 8,173,732 to 6,998,332 bytes per call, but a full scan changes
15.131 to 16.428 ms. This supports the simpler bounded LRU replacement in that
workload; it does not establish a production hit rate or RSS reduction. Relay
prefix reuse removes one observed singleton allocation (320 bytes/two calls to
256 bytes/one call). Allocation instrumentation is excluded from final timing.

The [packing comparison](#packing-comparison) exposes the selector's elapsed and
allocation costs relative to develop, its fee/result differences, and all ten
prospective optimization outcomes. Publisher polling, broad persistent aggregate
caching and index sorting were not adopted where observed
benefit or integration cost failed to justify them. Fixed 1,024 shards were rejected
at the user's direction after source-cost review; no native speedup is claimed.
The candidate index and source-bound decisions preserve all explored alternatives.

The pool occupies 16,005 physical Rust lines in 45 production files, including
inline tests, versus the accepted 15,319-line/42-file reference. Separate tests,
shared-crate changes and measurement tools are disclosed outside both counts.
No logic was moved or compressed to hide that cost. The maintained design owns
original observations, atomic coupled commits, bounded retained/transient work,
and ordered effect obligations through the actual producer/consumer paths. Shared
quotas, FIFO publication, shard collisions, full-population operations and caches
remain maintenance and resource costs. Synchronous callbacks/providers must return
for their joins to complete.

The engineering recommendation is to proceed with the complete refactor for these
contracts, capacity and substantial cheap-script/dependency throughput benefits,
accepting the measured RSS and CPU costs. It is not a recommendation based on the
small secp gain alone. The intentional Rust API change requires a SemVer-major
release relative to the published crate; upgrading existing configuration and
persistence remains supported. Downgrade conversion and parent-directory fsync
are outside the declared contract. Independent maintainer approval and release
decisions remain separate from this implementing-agent assessment.

## Verification and portable evidence

The final executable inputs pass strict workspace all-target Clippy with repository
lint/features, 1,422 isolated Nextest tests including ignored tests (no skips or
leaked pipes), 46 doctests, and all 176 release integration specs without retries.
RPC Markdown regeneration is byte-identical. The 75 Python checks remain applicable
to unchanged measurement scripts, and seven attribution-parser checks cover native
v2/v3 handling and rejection. Rust formatting, documentation links and executable
source identity are checked separately after reporting edits.

The delivery packet `tx-pool-g0-20260911-evidence.tar.gz` contains a manifest/index,
raw attempts and failures, original plans/tools, source archives including gitlinks,
binary hashes and binaries, validation receipts, optimization decisions and
independent replay. Extract it and run `python3 -B replay.py` from the packet root.
Replay verifies hashes and reconstructs observations, corpora, complete attempt
order, replicate arithmetic and every original qualification; it does not execute
the native binaries or recreate operating-system behavior. The separately supplied
`.sha256` file identifies the archive without embedding a self-referential hash.

The earlier `final-delivery-cdfde29e-r1` packet is retained unchanged inside the
delivery evidence. Its archive SHA-256 is
`f68e7b3c33d2deb1b07f8877a359b20b755cc87b99410931625b63414fa21e31`.
That earlier study retained 2,600 executions: 2,593 successes and seven rejections;
only three of eight rows qualified. Its secp throughput ratio 0.9810 and CPU ratio
1.0324, clock failures, fanout reset/timeout and RBF RSS precision/equivalence failure
remain historical results. The new protocol and measurements supersede their use
for current-source claims without erasing those failures.

Finite evidence does not establish a mathematical global optimum, cross-platform
performance, production workload frequencies, whole-node RSS bounds or independent
human acceptance. Every planned final scenario has a qualified performance or
explicit functional result; reorg CPU precision and focused packing equivalence
limits remain visible rather than being converted into passes.
