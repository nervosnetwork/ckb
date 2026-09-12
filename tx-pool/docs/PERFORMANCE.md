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

## Owner-account preparation

The subsequent owner-account refinement compares frozen `f9e299114` with
`ac5ad9133`; only production budget preparation differs. One temporary ordered
map aggregates old/new charges before producing the existing reservation lists.
Stack routing replaces temporary per-owner maps in recovery selection. It leaves
the live ledger, reservation/rollback and protected commit work unchanged. Actual
fixed-account and retained-map alternatives were measured and reviewed; the
selected scratch-map representation keeps fewer accounting concepts and the
original settlement behavior, at the cost of some potential allocation reduction.

Allocation-only production builds ran six balanced pairs per scenario, with eight
workers and four peers. These are median paired candidate/baseline ratios of
allocation traffic during the target terminal window, not retained memory:

| Workload | Allocation calls | Requested bytes |
|---|---:|---:|
| RBF, 16,384 target and warm transactions | 0.99126 (−0.874%) | 0.999988 (−0.0012%) |
| Reverse fanout, 16,640 target transactions | 0.98875 (−1.125%) | 0.999949 (−0.0051%) |

Separate uninstrumented A/A and A/B studies each used 12 balanced pairs, two fresh
processes per side/sample, and the preset 2% A/A, 1.5% paired-MAD and 4% interval-width
gates. Each study completed 100 successful executions including validation runs.
RBF uses 32,768 target/warm transactions for timing; reverse fanout keeps 16,640.
The table gives median paired ratios and pointwise 95% order-statistic intervals:

| Workload | Target CPU | Throughput | Mean lifetime peak RSS | Qualification |
|---|---:|---:|---:|---|
| RBF | 0.99681 [0.99320, 1.00263] | 0.99754 [0.98494, 1.02294] | 0.99672 [0.99380, 1.00562] | A/A and A/B quality pass; all directions unresolved |
| Reverse fanout | 1.00240 [0.98099, 1.01195] | 1.00533 [0.96914, 1.03006] | 0.99865 [0.99051, 1.00086] | A/A CPU/throughput and A/B throughput do not qualify |

The refinement has a measured reduction in allocation calls and a simpler
preparation representation. It does not establish faster execution, lower RSS,
or performance equivalence. In particular, successful executions do not override
the fanout noise failure. No gates were widened or studies rerun until favorable.
The intervals assume stable independent sampling; this protocol does not prove
that assumption or simultaneous coverage across metrics.

The accompanying test refinement passes 1,437 distinct workspace Nextest tests
(the normal suite plus its two ignored RPC cases). All 89 selected release
integration specs pass in one run without retries. Strict workspace Clippy
and scoped checks cover the changed source. The 3,136 phase/source transitions and
real-planner sequence check complete owner, projection, queue and account
populations; individual owner sizing deliberately shares the production formula.
Targeted mutation checks detect a previously missed complete-capture merge guard.
Six final generated mutants are caught; two comparator mutations cannot compile
because `Ordering` has no `Default`. Nine separately labeled seeded mutations,
including valid comparator counterparts, are caught. These bounded checks do not
establish a whole-pool mutation score. [Review guidance](REVIEW_GUIDE.md#maintain-test-value)
records how to preserve scenario intent and distinguish asynchronous completion.

The project evidence directory `resource-test-refinement-20260911` retains the
frozen source/binary/build receipts, native attribution, all three allocation
comparisons, `uninstrumented-final-{aa,ab}.json`, commands and complete attempts.
The rejected profiling-plus-allocation pilot and stale-binary build attempt remain
marked invalid and do not contribute measurements. [The benchmark protocol](BENCHMARK.md)
defines the metric windows, frozen inputs and replay limits.

## Composed stability and state sequences

The subsequent validation refinement uses frozen candidate
`09b9a634ef02d77b2e44062934f5a7837650900e` on production basis
`6ab298e06ba5c65bf6b60c0a0efb69ab31c04d23`. It adds composed service and generated
state tests, consolidates genesis fixtures and shares the existing OS memory probe.
Production behavior is unchanged. The [maintenance commands](MAINTENANCE.md#repeat-composed-workloads-and-replay-sequences)
describe the exact test boundaries and replay format.

The extended state suite passes 64 fixed seeds of 4,096 legal operations:
262,144 steps across receive/promotion, resolution, admission, removal, expiry,
clear, attach and detach. It checks complete Store populations and stale-cut
rejection after every operation. An isolated missing-queue-removal perturbation
is detected and shrinks from four commands to two; that exact trace fails under
the perturbation and passes after restoration. Retaining a retired owner and
skipping local publication waiting are also detected. These are three bounded
hand-seeded faults, not a whole-pool mutation score or exhaustive state coverage.

One service generation completes 4,096 mixed rounds in 1,531.17 seconds, with
450,560 remote pressure admissions, 4,096 capacity refusals, zero bans and
81,920 distinct canonically verified transaction hashes. Each round settles exact
callback/relay populations, checks observed owner/transaction/resolution release,
checks empty Store/account/active/outbox state and demonstrates same-peer recovery.
The 1,175,552 weak-owner observations can repeat identities; they do not count
unique allocations. The fixture supplies chain snapshots and live-cell updates,
without running whole-node consensus, network relay or mining.

All 4,096 observations per operation are retained. Values below are milliseconds
from the instrumented test build; local and attach responses include the controlled
callback hold and are not production latency estimates or comparative benchmarks.

| Operation | p50 | p95 | p99 | Maximum |
|---|---:|---:|---:|---:|
| Public local parent response | 4.799 | 6.833 | 9.661 | 43.917 |
| Chain attach response | 1.270 | 1.500 | 1.711 | 2.539 |
| Chain detach response | 0.456 | 0.573 | 0.672 | 1.939 |
| Detached recovery complete | 0.975 | 1.280 | 1.442 | 2.236 |
| Final clear response | 0.708 | 0.798 | 0.880 | 1.722 |

RSS grows from 91.203 MiB at service readiness to 192.875 MiB after round 64,
352.297 MiB after round 4,096 and 352.344 MiB after joined shutdown. It still grows
about 20 MiB across the final quarter's checkpoints: there is no observed plateau
or process-wide leak-free conclusion. The fixture retains an old database snapshot,
RocksDB write history and some block metadata; bounded verification/Store caches
and allocator/runtime retention also remain. A retained database log reports
12,793 writes and 630K keys at 1,200 seconds, with no flush or compaction. This does
not isolate the cause of RSS growth. Logical release and process memory trends
are separate findings.

Frozen-input validation passes strict pool all-target Clippy with repository lints and
internal/profiling/allocation-observation, 367 normal pool Nextest tests, both
extended tests and 59 Python measurement gates. The tests reuse the existing full
population oracle; individual owner sizing still shares the production formula.
Initial fixture/order failures and the first unsuccessful shrink acceptance remain
in the evidence, with their corrections and scope recorded.

The final CI check found three new test-counter uses of `Relaxed`; they were
changed to `SeqCst`, then the unchanged CI gate, strict pool Clippy and the short
mixed workload passed again. This module is compiled only under `cfg(test)`, so
the production benchmark inputs are unchanged. All extended observations above
remain bound to `09b9a634`; no latency or RSS observation from the later counter
revision is claimed. The packet records both source maps and the exact three-use
correction, reusing the other tests' unchanged relevant inputs.

The separate production-profile comparison uses prepared baseline
`d5d59579bcf9898aad60c9a8aa89218f097df808` with the same memory-helper move and
candidate `09b9a634`. Both contain identical production behavior and benchmark
helpers. All 14 native gates pass, including two expected malformed-cohort
refusals. Ascending calibration retains nine populations/18 pilots before fixing
four workloads at eight workers and four peers. Each study prescribes 12 balanced
pairs of two fresh processes per side/sample, with 15 s initial and 3 s per-attempt
cooldown. Original 0.25 s window, 1.5% paired-MAD, 4% interval-width and ±2% A/A
margins remain unchanged.

A/A completes 200 successful executions. A/B retains 189 successes and one RBF
baseline failure; its remaining ten RBF attempts are not started under the
runner's stop-on-failure rule. Both studies exit 2. No row qualifies under both
its A/A control and A/B rules, so all ratios below are descriptive and establish
no performance ranking or equivalence. Brackets are pointwise conservative 95%
median-ratio intervals, conditional on stable independent sampling; those
assumptions and simultaneous matrix confidence are not established.

| Workload | Target / warm | Throughput ratio | CPU ratio | RSS ratio | A/A | A/B |
|---|---:|---|---|---|---|---|
| `always_success` | 32,000 / 1,000 | 0.99032 [0.97020, 0.99642] | 1.00891 [1.00471, 1.01298] | 0.99137 [0.97969, 0.99593] | aa_equivalence_unresolved | comparable |
| `fanout_ready_64_reverse` | 8,320 / 0 | 1.00374 [0.92849, 1.04039] | 1.00349 [0.98694, 1.01705] | 1.00142 [0.99957, 1.01385] | noisy | noisy |
| `rbf_pairs` | 32,768 / 32,768 | — | — | — | aa_equivalent | non_comparable |
| `reorg_in_flight` | 2,000 / 100 | 1.00001 [0.99703, 1.00148] | 0.99978 [0.98518, 1.02082] | 0.99703 [0.99654, 0.99895] | imprecise | comparable |

The always-success A/A throughput interval reaches 1.02221, outside the 1.02 upper
margin. Fanout fails the throughput noise gate in both studies. Reorg A/A CPU
interval width is 4.005%, just above the unchanged 4% limit. Their successful
executions do not override these failed controls.

The RBF baseline failure occurs at pair 10, replicate 1. It records 65,129 accepted
hashes and 32,768 rejected hashes, with 32,361 in both sets: 407 replacement targets
received refusal without acceptance. The union covers all 65,536 planned hashes;
there are no unresolved hashes, duplicates or generation resets. Warm membership
and relay results are validated before replacement submission. The 120-second
acceptance wait therefore cannot reach its all-accepted contract. Remote batch
processing can successfully complete while individual transactions receive policy
refusals; it does not promise accepted membership. The retained relay records do
not include rejection reasons, so quota pressure is a possible explanation, not
a uniquely established cause. This is a failed workload contract, not evidence of
a deadlock or an isolated candidate regression. Partial successful RBF pairs are
not converted into a ratio, and the failed attempt is not replaced.

`tx-pool-stability-sequences-20260912-evidence.tar.gz` preserves all actual attempts,
source/VM/build identities, native benchmark binaries, test fingerprints, original
long-run observations, the final short-test binary, seeded faults and numerical
replay. The original long-test executable was not copied before the test-counter
rebuild; its source, original fingerprint and logs are retained. The final short
binary is identified separately. Replay checks source maps, the exact counter
correction, raw records, attempt order, arithmetic and original qualifications;
it does not rerun native tests or recreate OS behavior.

## Rejection diagnostics and repeated refinement

The follow-up addresses the missing evidence in the earlier 407-refusal RBF
failure. Twelve exact diagnostic repetitions succeeded, while a controlled
public-service backlog reproduced `Full("peer pipeline")`: raw submission can
finish before resolved-owner admission encounters capacity. All refused victims
remained accepted, and retries through the same peers succeeded. This establishes
a retryable mechanism, not the unique cause of the historical failure. The
observed remote tree matched the pre-diagnostic production baseline; neither
those controls nor the earlier failed A/A establish a newly introduced regression
or measured equivalence.

One bounded rejection record now carries the reason used by metrics, optional
recent storage and post-commit diagnostics. Transient refusals retain their cause
through publication without becoming persistent transaction status. Candidate
context holds source, phase, peer and up to five account observations, without
retaining a transaction or owner. The resource snapshot is labelled rejection
preparation: concurrent work may have changed usage since the refusing operation.
Diagnostics run after commit and guard release; expected RBF victim callbacks
remain separate from rejected candidates.

Read-only type inspection of preserved arm64 test binaries measures the inline
effect at 488 bytes before and 496 bytes after this change. Candidate rejection
context occupies 528 bytes in its separate allocation; its accounting includes
an additional allocator allowance. Retained diagnostic strings are bounded and
charged. These are representation costs, not measurements of allocation traffic
or whole-process RSS.

The benchmark streams rejection and service-warning/error records into its
retained output and checks a final capture count after runtime cleanup. A real
prototype completed its pressure workload but emitted a persistence error after
the old capture boundary; that attempt remains failed. Both adapters now request
exit, release caller handles, wait for the existing runtime task guards and join
the relay observer, using an isolated persistence file. The target timer remains
unchanged; ancillary shutdown time and lifetime peak RSS include that cleanup.
Profiling capture and later reanalysis enforce the same diagnostic integrity.
A separately preserved `prod` executable with `profiling` also passes actual
Samply CPU capture, independent span capture and artifact reanalysis for 32,000
always-success targets. Reanalysis reproduces the exact summary hash; this checks
the logger/subscriber lifecycle and does not rank production performance.

Frozen candidate `bc6f9d97f36a30928a9bfa49039b4a08502601e4`, baseline
`a599af3c0da8891ef35c48be395b15f2272051c7` and legacy adapter check
`3ee6a809db5883c17c267a01f11b44c126f68454` pass 16, 15 and 15 native gates.
All nine shared successful scenarios have identical corpus identities across
the three binaries. Candidate pressure records 2,787 peer-pipeline refusals,
retains every refused victim and accepts all same-peer retries, ending with
exactly 32,768 replacements. The baseline without the new diagnostic producer
correctly fails the negative collection check and preserves 2,786 refusal-only
hashes with explicitly missing causes. That check validates failure detection;
it does not turn the baseline pressure workload into a success.

Repeated refinement also removes producer-side fixture references that outlived
their purpose. The mixed workload keeps exact callback/relay expectations and
weak observations of the production owner, transaction and resolution. The
sequence oracle uses one forward pass because corpus parents have lower IDs and
settlement leaves accepted membership unchanged; the production wake loop still
handles its actual dynamic work. Independent read-only reviews traced these
relationships and the rejection and shutdown paths. The final refined library
passes 370 isolated Nextest tests with two extended tests excluded from that
normal run; strict all-target Clippy and all 80 Python script checks pass.

Both extended tests then pass using those previously copied executables, with
`internal,profiling` in the test profile. All 64 seeds complete 262,144 legal
operations and exercise every operation kind. The mixed run completes 4,096
rounds in 1,549.139 seconds, covering 81,920 distinct verified transaction hashes,
450,560 accepted pressure bodies, 4,096 capacity refusals and 1,175,552 successful
release observations. Observation identities can repeat; the last count is not
a count of unique allocations. Exact terminal, same-peer retry and joined cleanup
checks pass. Three isolated seeded faults still fail for their intended reasons:
stale queue membership, a retained retired owner and premature local completion.
The queue failure shrinks from four commands to two, reproduces with the faulty
binary and passes with the healthy binary. Each actual faulty executable was
also preserved before use, and the restored controls pass.

The final mixed run's released-checkpoint RSS grows from 150.31 to 286.25 MiB;
joined RSS and the observed lifetime peak are 286.28 MiB. The last quarter grows
from 265.94 to 286.25 MiB, so this run does not show a stable RSS plateau. Logical
release does not attribute the remaining process memory among database state,
caches and allocator retention. Unrelated compilation and fuzz activity was
observed during the run; these instrumented latency values are diagnostic and
do not compare performance against the earlier source. All samples and all
67 trace records remain available for numerical replay.

| Final mixed-run observation | p50 ms | p99 ms | Maximum ms |
|---|---:|---:|---:|
| Local parent response | 4.473 | 11.904 | 43.199 |
| Chain attach response | 1.200 | 3.177 | 11.829 |
| Chain detach response | 0.415 | 1.469 | 8.290 |
| Detached recovery complete | 0.836 | 3.895 | 18.390 |
| Final clear response | 0.672 | 1.101 | 6.088 |

The final incremental benchmark compares the bounded rejection changes with the
unchanged `178669c72` production baseline through the same final harness. Its four
rows cover always-success transactions, reverse ready fanout, windowed RBF and
in-flight reorg, all with eight workers and four peers. Prospective ascending
calibration selects 32,000/1,000, 16,640/0, 32,768/32,768 and 2,000/100 target/warm
populations respectively. All 20 calibration attempts remain, including the
smaller populations whose target windows were too short.

Candidate same-binary A/A precedes baseline/candidate A/B. Each row has two pilots
and 24 balanced pairs with four fresh-process replicates per side: 194 attempts
per row and study. The frozen protocol keeps the original 0.25-second minimum
target window, 1.5% paired throughput relative MAD, 4% primary ratio interval-width
limit and A/A interval containment within 0.98–1.02. These are pointwise 95% median
ratio intervals under stable, independent sampling assumptions (the exact
24-pair order-statistic coverage is 97.734%). The procedure does not prove those
assumptions or simultaneous matrix coverage. Qualification needs
both studies' gates. The complete joined shutdown and lifetime RSS scope are
common to both sides. Windowed RBF measures admitted progress and does not replace
the preserved burst workload or the separate overload/refusal checks.
All four A/A rows pass those original quality and equivalence gates. All four A/B
rows also pass, with 776 successful native attempts per study and no failures or
replacement attempts. The 20 calibration executions bring the total to 1,572.
Ratios below are medians of the 24 paired candidate/baseline ratios, with the
original intervals. CPU covers the target window; RSS averages the four fresh
process lifetime peaks within each side's sample, including joined cleanup.

| Workload | TPS ratio [interval] | CPU ratio [interval] | RSS ratio [interval] |
|---|---:|---:|---:|
| Always success | 1.0066 [1.0002, 1.0092] | 0.9962 [0.9917, 0.9988] | 1.0150 [1.0143, 1.0187] |
| Reverse ready fanout | 0.9932 [0.9895, 0.9991] | 0.9985 [0.9921, 1.0028] | 1.0008 [1.0004, 1.0014] |
| Windowed RBF | 0.9974 [0.9882, 1.0059] | 0.9995 [0.9974, 1.0041] | 1.0022 [0.9992, 1.0046] |
| In-flight reorg | 0.9995 [0.9985, 1.0005] | 0.9932 [0.9892, 0.9985] | 1.0017 [1.0011, 1.0023] |

Always-success throughput increases by 0.66% and target CPU decreases by 0.38%,
with a 1.50% RSS cost. Fanout throughput decreases by 0.68% and RSS increases by
0.08%; its CPU interval spans one. Reorg CPU decreases by 0.68% and RSS increases
by 0.17%, while its throughput interval spans one. All three RBF intervals span
one. These small workload-specific changes do not establish a general speedup or
attribute RSS differences to a particular allocation. The functional benefit is
bounded, complete refusal evidence and a verifiable harness lifecycle; the
measured costs remain part of that engineering tradeoff.

`tx-pool-rejection-refinement-20260912-evidence.tar.gz` preserves the frozen source
archives, actual binaries, original failures, native/profile captures, extended
observations and every calibration/formal attempt. Its offline replay checks file
and source identities, reconstructs the complete attempt schedule and original
qualifications, and recomputes extended-run statistics and the profiling summary.
It also retains the preceding immutable packet, including the unresolved
historical refusal evidence. Replay starts no native program and does not recreate
operating-system behavior.

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
earlier complete-refactor production comparison measures those combined changes.

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

The current pool occupies 16,777 physical Rust lines in 49 production files,
including inline tests, versus the accepted 15,319-line/42-file reference. The
owner-account refinement adds 58 lines over its `f9e299114` baseline: two in
production budget preparation and 56 in test-only observation interfaces.
The subsequent composed-validation refinement adds 15 cfg(test) observer lines
in those files; the sequence and stability workloads live in separate tests.
The bounded rejection diagnostics add 157 production lines for retained context,
account observations and publication through the existing effect lifecycle.
Separate tests, shared-crate changes and measurement tools are outside both counts.
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
RPC Markdown regeneration is byte-identical. That migration also passed 75 Python
checks and seven attribution-parser checks for native v2/v3 handling and rejection.
The changed measurement paths now pass the 80-check suite and native/profiling
gates described above. Rust formatting, documentation links and executable source
identity are checked separately after reporting edits.

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
human acceptance. Every planned migration scenario retains its qualified
performance or explicit functional result; earlier reorg CPU precision and focused
packing equivalence limits remain visible rather than being converted into passes.
