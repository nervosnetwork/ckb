# Transaction-pool performance evidence

The earlier migration study improves all seven qualified receive-to-terminal
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
own final source and direct develop comparison. Later executable changes require separate measurements
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

The final packing source is `8aec0d70bc0e86f6bb86e6dfa4aac07e658c662f`.
It is compared directly with prepared develop `9eaf18b24a8d7b4fef7666f711585d72fd604132`,
whose production basis is `95fd03933ce2a396b84aed12caaa0f984165cd6c`.
Across the 20 cases, the direct A/B intervals establish lower selection time in 13, higher time in 7, and leave 0 unresolved.
These are selector costs on this host, not complete block-template latency or
receive-to-terminal throughput. No global optimality or universal advantage is claimed.

The final implementation borrows captured owners and protocol fields, compiles
one numeric causal graph, evaluates single-parent totals directly, reuses one
exact closure at compatible merges, and uses bounded initial/modified priority
queues. Remaining-chain ordering and consumer-aware descendant work avoid
unnecessary traversals. The [execution description](architecture/EXECUTION.md#ordered-publication-and-projections)
states ownership, invalidation, ordering and resource bounds.

### Direct develop A/B contract

The complete matrix is five shapes × equal/CPFP fees × All/Partial, with 16,384
entries and K=64. All fits the fixture; Partial allows 595,000 bytes and
3,500,000,000 cycles. Chains are bounded cohorts, not one 16,384-entry chain.
The production default ancestor limit is 1000; the separate completed K1000
functional observations do not establish a develop performance ranking.

Each case uses 12 independent paired process blocks, in the originally fixed
balanced/interleaved arm order: 480 formal native runs. The statistic is each
process mean of individually timed calls, with calibrated repetition counts
held fixed before A/B. All per-process semantic checks and the minimum 20 ms
accumulated-exposure and clock-bracket gates remain in force. Intervals are
conservative pointwise 95% sign/order-statistic intervals for the paired median
ratio; they do not provide simultaneous coverage or prove process independence.

At the user’s direction, acceptance uses direct paired A/B without requiring
a separate A/A equivalence gate. Previously collected A/A and intermediate
observations remain diagnostic; their original plans and qualifications are
unchanged. Remaining A/A, intermediate-version and extra cache/boundary timing
runs were canceled before the first A/B capture. No result was excluded based
on its direction. This new process-mean contract also leaves every original
v2 per-call 1 ms qualification intact; short individual calls do not become old-gate passes.

The [timed window](BENCHMARK.md#measure-template-transaction-selection) includes
current cold graph construction, selection and selection-state destruction.
Develop selects from its admission-maintained PoolMap, whose maintenance is
outside the window. Both call their real production selector. Fixture setup,
Store capture/locks, optional content, final DAO and complete template building
are excluded. The current cold adapter does not use the template-loop graph cache.

| Shape / fees / budget | Develop ms | Final ms | Final/develop [95% interval] | Decision |
|---|---:|---:|---|---|
| independent / equal / All | 5.053 | 3.563 | 0.7062 [0.6527, 0.7358] | Lower time |
| independent / equal / Partial | 0.665 | 1.378 | 2.0652 [2.0270, 2.0957] | Higher time |
| independent / cpfp / All | 5.214 | 3.553 | 0.6875 [0.6526, 0.7262] | Lower time |
| independent / cpfp / Partial | 0.660 | 1.388 | 2.0958 [2.0071, 2.1556] | Higher time |
| chain / equal / All | 53.595 | 3.449 | 0.0647 [0.0590, 0.0733] | Lower time |
| chain / equal / Partial | 6.585 | 2.199 | 0.3297 [0.3075, 0.3566] | Lower time |
| chain / cpfp / All | 54.009 | 3.295 | 0.0616 [0.0583, 0.0704] | Lower time |
| chain / cpfp / Partial | 6.497 | 2.156 | 0.3359 [0.3245, 0.3572] | Lower time |
| fanout / equal / All | 266.102 | 5.939 | 0.0222 [0.0209, 0.0239] | Lower time |
| fanout / equal / Partial | 46.302 | 3.038 | 0.0656 [0.0619, 0.0693] | Lower time |
| fanout / cpfp / All | 12.128 | 5.592 | 0.4356 [0.4118, 0.4668] | Lower time |
| fanout / cpfp / Partial | 1.157 | 2.288 | 1.9559 [1.8053, 2.1704] | Higher time |
| diamond / equal / All | 9.390 | 4.984 | 0.5400 [0.5273, 0.5948] | Lower time |
| diamond / equal / Partial | 0.987 | 2.656 | 2.6523 [2.4340, 2.8100] | Higher time |
| diamond / cpfp / All | 8.887 | 4.674 | 0.5449 [0.5223, 0.5697] | Lower time |
| diamond / cpfp / Partial | 0.949 | 2.639 | 2.7247 [2.5510, 2.7513] | Higher time |
| mixed / equal / All | 12.807 | 5.030 | 0.3944 [0.3740, 0.4416] | Lower time |
| mixed / equal / Partial | 1.294 | 3.012 | 2.3538 [2.3059, 2.4398] | Higher time |
| mixed / cpfp / All | 13.351 | 5.232 | 0.3878 [0.3685, 0.4328] | Lower time |
| mixed / cpfp / Partial | 1.390 | 3.190 | 2.2947 [2.1599, 2.3648] | Higher time |

Absolute times are medians of process means. Paired-ratio medians need not
equal ratios of those separate absolute medians. Lower elapsed ratios are better.

### Selected work and quality

Every final-source call reproduces its case’s exact selected set, order, fees,
bytes and cycles. All-fit calls select all 16,384 transactions. Develop may emit
different valid partial results under its legacy tie policy. The table retains
all observed partial transaction/fee ranges, rather than asserting equal work.
Complete byte/cycle utilization and set/order variation remain in the packet.
Large-fixture optimality is not inferred from fee totals or utilization.

| Shape / fees | Selected transactions: develop → final | Selected fees in shannons: develop → final |
|---|---:|---:|
| independent / equal | 1,919 → 1,919 | 594,890,000 → 594,890,000 |
| independent / cpfp | 1,919 → 1,919 | 594,890,000 → 594,890,000 |
| chain / equal | 1,919 → 1,919 | 594,890,000 → 594,890,000 |
| chain / cpfp | 1,919 → 1,919 | 904,859,000 → 904,859,000 |
| fanout / equal | 178 → 178 | 590,426,000 → 590,426,000 |
| fanout / cpfp | 1,453 → 1,473 | 44,300,517,760 → 44,951,454,520 |
| diamond / equal | 1,852 → 1,852 | 594,492,000 → 594,492,000 |
| diamond / cpfp | 1,852 → 1,852 | 16,394,505,900 → 16,394,505,900 |
| mixed / equal | 1,780 → 1,780 | 594,505,000 → 594,505,000 |
| mixed / cpfp | 1,780 → 1,780 | 8,529,892,570 → 8,529,892,570 |

### Resource observations

Separate instrumentation records final per-selection allocation traffic; it
never supplies timing claims. Across the 20 final cases the complete calls and
requested bytes remain in `final-cost-12/rows.json`. Stable template-graph reuse
retains 3,408,048–3,702,960 requested bytes after captured owners retire. These
are numeric graph storage and weak Entry allocations; retired transaction and
resolved payloads are released. This is neither allocator overhead nor RSS.
One-owner or whole-source renewal rebuilds the graph and adds 131,072 requested
bytes versus cold construction for the weak-identity vector.
Repeated complete-template RPC reads do not establish graph-cache hit traffic.

The following fixed 64-call CPFP observations use kernel wait4 CPU and maximum
RSS over the entire native child, including fixture/source setup, preflight,
warmup, selection and destruction. They are descriptive whole-process costs,
not isolated selector CPU/RSS, node RSS, or a statistical resource ranking.

| Shape / budget | Total CPU seconds: develop → final | Lifetime peak RSS MiB: develop → final |
|---|---:|---:|
| independent / All | 0.730 → 0.659 | 128.06 → 112.00 |
| independent / Partial | 0.183 → 0.214 | 128.61 → 112.27 |
| chain / All | 4.260 → 0.715 | 132.59 → 120.33 |
| chain / Partial | 0.798 → 0.286 | 132.72 → 120.27 |
| mixed / All | 1.398 → 0.886 | 141.03 → 122.22 |
| mixed / Partial | 0.256 → 0.387 | 139.41 → 126.86 |

The final-source strict workspace/all-target Clippy, 1,448 Nextest tests including
ignored tests, 46 doctests and 176 release integration cases pass; integration
needed no retries. Six cache diagnostic modes each pass 12 contract tests.
The final report reuses these checks because only documentation changes after
the frozen executable inputs. Mutation checks reject missing fork fallback,
incorrect closure reuse and unsafe budget-exhaustion shortcuts.

The `tx-pool-packing-20260911-evidence.tar.gz` packet retains the direct A/B plan, all 480
captures, original qualifications, canceled-study data, sources/binaries,
validation receipts and the 76 completed resource plus 12 boundary observations.
Replay checks raw logs, source/binary receipts, complete case membership and
statistical arithmetic; it does not recreate OS behavior. Historical
`packing-r28`/`packing-r29` remain in the earlier migration packet and do not
supply final-source comparisons.

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
allocation costs, direct develop comparisons, fee/result differences and all 20
final A/B outcomes. Publisher polling, broad persistent aggregate
caching and index sorting were not adopted where observed
benefit or integration cost failed to justify them. Fixed 1,024 shards were rejected
at the user's direction after source-cost review; no native speedup is claimed.
The candidate index and source-bound decisions preserve all explored alternatives.

The pool occupies 16,488 physical Rust lines in 47 production files, including
inline tests, versus the accepted 15,319-line/42-file reference. This packing change
adds 347 lines over its 16,141-line baseline. Separate tests,
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

The earlier migration executable inputs pass strict workspace all-target Clippy with repository
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
