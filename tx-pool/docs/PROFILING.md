# Profiling and development tools

Use profiling to locate work and test causal hypotheses. Use the
[paired benchmark](BENCHMARK.md) to measure a frozen change. Both execute
[profile_one_shot](../benches/profile_one_shot.rs), which drives the production
service and validates exact callback/relay terminals.

## Choose the observation

| Question | Tool or feature | What it observes |
|---|---|---|
| Where is target CPU attributed? | [profile.py](../scripts/profile.py) + Samply | Sampled stacks cropped to the target window |
| Which pool paths execute, and how often? | `profiling` + `TX_POOL_PROFILE_TRACE_PATH` | Entered-scope wall time and counts in a separate run |
| Which tasks poll, wait or wake repeatedly? | `tokio-trace` + Tokio Console | Runtime scheduling events; busy time is not CPU time |
| Does a change reduce allocation traffic? | `allocation-observation` through the comparison runner | Allocation calls and requested bytes inside the target window |
| Does the change improve production behavior? | Uninstrumented A/A and A/B | Throughput, target process CPU and process-lifetime peak RSS |

Default builds enable none of these diagnostics. `tokio-trace` includes
`profiling`; setting the span-output environment variable enables the additional
pool recorder. Instrumented results do not rank uninstrumented production timing.

## Capture CPU and pool spans

Prerequisites: a POSIX host, Python 3.11+, the repository Rust toolchain/build
dependencies and `samply` on `PATH` (`cargo install --locked samply` if absent).
Check `samply --version` and
`python3 tx-pool/scripts/profile.py capture --help`. Run from the repository root
and keep artifacts outside the source tree:

```sh
capture_dir="$(mktemp -d /tmp/txpool-profile.XXXXXX)"
python3 tx-pool/scripts/profile.py capture \
  --output-prefix "$capture_dir/rbf" \
  --scenario rbf_pairs --target 16384 --warm 16384 \
  --workers 8 --peers 4 --rate 1000 --timeout-seconds 180

python3 tx-pool/scripts/profile.py analyze \
  --manifest "$capture_dir/rbf.manifest.json"
samply load "$capture_dir/rbf.json.gz"
```

The command builds a `prod` binary with `profiling`, captures a CPU run and then
an independent span run, verifies them and writes a deterministic summary. A
reused binary requires both `--binary /absolute/path/to/binary` and
`--binary-profile prod`. This flag is an explicit attestation, not a way to turn
a debug binary into a production build. `--target-dir` changes the isolated build
directory; the capture CLI has no arbitrary build-feature option.

Other useful workloads are `always_success`, `secp256k1`, `dependent`,
`dependent_reverse`, `dependent_forest_10`, `fanout`, `fanout_reverse`,
`reorg_in_flight` and `always_success_callback_500us`. Reverse workloads require
`warm=0`; RBF requires equal warm and target counts. The current fanout fixture
supports at most 5,752 transactions. The [benchmark guide](BENCHMARK.md#workload-integrity)
explains fixture and terminal constraints. Peer ranges are split into contiguous
batches using actual relay count/byte limits, preserving peer and cycle identity.

### Artifacts and replay

For prefix `rbf`, keep the complete bundle together:

| File | Use |
|---|---|
| `rbf.json.gz`, `rbf.json.syms.json` | Raw Samply profile and presymbolication sidecar |
| `rbf.stdout.log`, `rbf.stderr.log` | CPU-run markers, observations and diagnostics |
| `rbf.spans.json`, `rbf.span.stdout.log`, `rbf.span.stderr.log` | Independent instrumented run |
| `rbf.manifest.json` | Source/build/host/workload identity and artifact sizes/hashes |
| `rbf.summary.json` | Deterministic analyzed observations and hotspot tables |

Analysis verifies artifact paths, sizes and SHA-256 before consuming them. The
bundle can move and no longer needs the capture binary. Preserve the analyzer
and its [process helper](../scripts/measurement_process.py) with an immutable study;
use the matching analyzer for each bundle schema. Current manifest/summary/window
schemas are 8/7/2, and current span output is schema 4. Reanalysis is not a new
timing run.

### Read the result correctly

The target window starts immediately before submission and ends at the required
callback/relay terminals. Fixture generation, warmup, exact-set checking, p99
sorting and ordinary shutdown are outside it. In-flight reorg deliberately overlaps
the window. CPU and span runs have different schedules and cannot be combined
into one execution timeline.

| Observation | Interpretation and limit |
|---|---|
| Leaf CPU weight | Statistical assignment of the preceding observed CPU interval to its ending stack |
| Inclusive CPU weight | Includes child stacks; rows overlap |
| Top 100 hotspots | Truncated table; weights need not sum to all attributed CPU |
| Missing CPU delta/stack | Missing observation, not zero work; inspect coverage fields |
| `authority.apply` | Reservation, support preparation, guard acquisition, validation, mutation and cleanup |
| `authority.acquire` | Sequential acquisition/bookkeeping, partly with guards already held; not pure lock wait |
| `stage.verify` | Entered verifier-driver scope; separately spawned VM work is outside that driver scope |
| `effects.publish` | Synchronous endpoint work; a blocked callback can increase wall time without consuming CPU |

Schema 4 records starts, entries, active counts at capture boundaries and cumulative
entered wall time clipped to its own window, including scopes entered before it.
Suspended async intervals are excluded; blocking/descheduling inside an entered
scope remains included. Nested/concurrent entries overlap.

CPU analysis includes only complete intervals whose preceding sample is already
inside the window. Inspect observed-CPU coverage and missing-stack weight before
attribution. A waiting stack is not proof of CPU spent waiting. Wall and monotonic
windows must agree within `max(1 ms, elapsed / 10,000)`.

## Observe tasks with Tokio Console

Install a missing client with `cargo install --locked tokio-console`, then check
`tokio-console --help`. Build explicitly with Tokio instrumentation, writing Cargo's
artifact records outside the checkout:

```sh
console_dir="$(mktemp -d /tmp/txpool-console.XXXXXX)"
RUSTFLAGS="--cfg tokio_unstable" CARGO_TARGET_DIR="$console_dir/target" \
  cargo build --locked -p ckb-tx-pool --bench profile_one_shot \
  --profile prod --features tokio-trace --message-format=json-render-diagnostics \
  > "$console_dir/build.jsonl"

console_bin="$(python3 - "$console_dir/build.jsonl" <<'PY'
import json, sys
paths = set()
for line in open(sys.argv[1]):
    record = json.loads(line)
    if (record.get('reason') == 'compiler-artifact'
            and record.get('target', {}).get('name') == 'profile_one_shot'
            and record.get('executable')):
        paths.add(record['executable'])
assert len(paths) == 1, paths
print(paths.pop())
PY
)"
TOKIO_CONSOLE_BIND=127.0.0.1:6669 PYTHONPATH=tx-pool/scripts \
  python3 - "$console_bin" <<'PY'
import sys
from measurement_process import run_process
result = run_process(
    [sys.argv[1], 'always_success', '4096', '512', '4', '4'], timeout=180)
raise SystemExit(result.returncode)
PY
```

In another terminal, start `tokio-console http://127.0.0.1:6669` before launching
the workload so it can attach promptly. RBF uses positional arguments
`rbf_pairs 2048 2048 4 4`. These are finite diagnostics; a short run can finish
before a client connects. `TOKIO_CONSOLE_BIND=127.0.0.1:0` selects a free port and
stderr prints `TOKIO_CONSOLE_BOUND` with the actual address. Invalid or occupied
addresses fail before workload execution.

To collect pool spans in the same diagnostic run, add
`TX_POOL_PROFILE_TRACE_PATH="$console_dir/spans.json"` to the run's environment.
Omit it for task-only observation: pool counters add a recorder mutex. The
subscriber starts before tasks; Console aggregation runs on an owned private
runtime. Its drain bypasses cooperative budgeting and has no per-poll bound,
which is why unattended runs use the process timeout above.

Inspect task polls, wakes and resource waits around the target window. For an
exported event stream, qualify dropped-event counters, clean stream termination,
metadata/statistics joins and window coverage; the interactive connection alone
proves none of these. Persistent-task totals may include warmup/shutdown. Captures
with dropped events or truncated streams cannot establish complete scheduling
observations. The 250 ms post-work grace is outside timing and
does not guarantee delivery completeness. This recipe covers the finite harness;
it does not establish complete-node Console qualification.

## Observe allocations

Use [cross_version_benchmark.py](../scripts/cross_version_benchmark.py) with the same
prepared roots/scenarios as a comparison, a separate output file and
`--allocation-observation enabled`. The runner builds with that feature, or a
supplied binary must already include it. Only allocation calls/bytes may rank
that experiment; the recorded timing and RSS remain diagnostic. Allocation bytes
are traffic, not retained memory or a leak measurement. Compare source/destination
sharing and lifetime with the [resource review](REVIEW_GUIDE.md#check-resource-composition).

## Investigate a performance problem

1. **Define the question.** Freeze source, toolchain, configuration, fixture and
   terminal contract. Check functional completion before attribution.
2. **Locate work.** Use Samply leaf/inclusive stacks and separate pool counters:
   reverse fanout exercises resolution/wake work; RBF exercises admission/Apply
   and victim notices; reorg exercises capture, invalidation and publication.
3. **Explain the mechanism.** Trace producers and consumers with `rg` and Git.
   Use allocation observations for copying/retention and Console for poll/wake
   behavior. Inspect compiled layout or assembly for a specific representation
   or protected-work question; a hot stack alone does not establish its cause.
4. **Test the cause.** Isolate one material change, preserve errors and ownership,
   and use event-driven regressions for affected contracts. For notification work,
   distinguish state changes that require wakeup from unrelated queue activity.
5. **Measure independently.** Rebuild uninstrumented binaries; run frozen A/A
   then A/B, retaining failures and adverse CPU/RSS observations. Review effect
   obligations, cache state and workload equivalence before assigning causality.

A correctness regression and a controlled performance comparison answer different
questions. The [review guide](REVIEW_GUIDE.md#development-and-review-method)
owns the wider engineering method.

## Failures and tool maintenance

| Failure | Action |
|---|---|
| Samply fails before any workload marker | Inspect stderr and host sampling permission; retain the failure and use suitable sampling permission for the owned diagnostic |
| No/short target samples | Check terminal completion and workload duration; select a prospectively declared larger valid population |
| Missing required spans | Verify the profiling build and trace output; a sequential per-transaction substitute is not production batch ingress |
| Hash/schema/clock failure | Preserve the bundle; investigate exact source/tool/clock mismatch instead of editing the receipt |
| Timeout or interruption | Preserve partial logs; the shared POSIX runner cleans descendants in its process group |

Builds have a 3,600-second bound and identity commands a 120-second bound.
Source/binary identities are checked around capture. Escaped process groups and
SIGKILL of the runner are outside its cleanup guarantee. An incomplete capture is
not a valid bundle. After editing scripts, schemas or instrumentation, run the
[maintenance gates](BENCHMARK.md#maintenance-gates), including tampering,
window, terminal and process-cleanup canaries.
