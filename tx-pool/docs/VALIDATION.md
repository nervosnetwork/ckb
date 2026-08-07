# Tx-Pool Validation and Machine Contracts

This document is the maintenance entry point for the architecture and review
contracts. Run commands from the repository root. Validators are read-only by
default; CI detects drift and never rewrites a proposed change.

The normative design is [`ARCHITECTURE.md`](ARCHITECTURE.md). The behavior and
test-facing contract is [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md).

## Machine-readable files

These files live in `tx-pool/` so local tools and CI can consume them without
parsing prose. Human-maintained files contain only semantic decisions. Paths,
symbols, enum variants, test names, selectors, commands, counts, module wiring
and Markdown tables are discovered, generated or checked.

| File | Authority and purpose | Consumer | Update rule |
|---|---|---|---|
| [`architecture-contract.json`](../architecture-contract.json) | Stable UAK vocabulary, T1-T13 obligations, executable mathematical proof policy and durable residual risks. The validator derives public builder/controller/relay-receiver methods plus owner/phase/status/boundary variants and identity anchors from Rust, binds each used model family and discovered-domain cross-cutting protocol to registered behavior evidence, and rejects disagreement; current release status is deliberately absent. | `check_security_manifest.py` | Edit only for an architecture decision; update `ARCHITECTURE.md`, behavior evidence and tests together. Never copy Rust enums or public methods into a validator. |
| [`review-behaviors.json`](../review-behaviors.json) | Stable `TP-*` rule/attack semantics, exact production-symbol owners, T1-T13 mappings and curated unit/integration evidence. Focused commands and the review table are generated from it. | `check_review_guide.py`, `check_security_manifest.py`, `check_test_layout.py` | Edit only when behavior, ownership or proof evidence changes. Every symbol and exact test must resolve; no command or count field is allowed. |
| [`integration-impact.json`](../integration-impact.json) | Curated complete set of registered process specs whose production paths cross tx-pool ingress, verification, pool mutation, relay, mining/template or transaction-bearing reorg boundaries. | `check_review_guide.py`, `check_security_manifest.py` | Hand-edit when a relevant spec is added, removed, renamed or its boundary changes. Integration CI checks it against `ckb-test --list-specs`. |
| [`security-regression-manifest.json`](../security-regression-manifest.json) | Assembly manifest binding package/features, architecture, behavior, integration universe, generated inventory and explicit release blockers. It stores no derived count or individual evidence. | `check_security_manifest.py` | Edit only when an assembly input or release decision changes. Test counts are always derived. |
| [`test-layout-manifest.json`](../test-layout-manifest.json) | Allowed dedicated test roots plus named irreducible test observation seams. Module wiring and `cfg(test)` sites are discovered from Rust rather than copied. | `check_test_layout.py` | Edit only for a deliberate directory boundary or exceptional seam. A current observation is never accepted merely by copying it into an allowlist. |
| [`test-inventory.txt`](../test-inventory.txt) | Exact sorted snapshot of discovered internal-feature Rust tests and the managed integration spec names. This is generated evidence, not a hand-written selection list. | `check_security_manifest.py` | Regenerate with `--update-inventory` after an intentional test/integration inventory change, then review the diff. |

The evidence direction is one-way:

```text
Rust source -------- discovers -------> owner/state/identity facts
nextest + ckb-test -- discovers -------> complete executable test universe
         |
         v exact reconciliation
architecture-contract.json -----------> T1-T13 vocabulary
review-behaviors.json -----------------> semantic rules + exact owners/evidence
integration-impact.json --------------> curated process boundary
         |
         +-- generates ----------------> REVIEW_GUIDE.md commands and tables
         +-- generates ----------------> test-inventory.txt
         +-- validates ----------------> test-layout exceptions and CI gates
```

The only manual layer is semantic: why a rule exists, its attack case,
compatibility and performance bounds, and whether a residual risk is accepted.
Everything mechanically derivable has one owner and is generated or checked.
CI is read-only and fails on dangling symbols, zero-match evidence, missing
T1-T13 coverage, unregistered relevant process specs, stale inventory,
test-code leakage or generated Markdown drift.

## Formal falsifier

The bounded permit/effect protocol has an independent TLA+ falsifier under
[`formal/`](../formal/). [`PermitEffect.tla`](../formal/PermitEffect.tla)
models fair permit handoff, bounded worker occupancy, effect pressure and
publisher progress. [`PermitEffect.cfg`](../formal/PermitEffect.cfg) must close
the complete finite state space with its safety and liveness properties.
[`PermitEffectReachability.cfg`](../formal/PermitEffectReachability.cfg)
deliberately checks a false invariant and must produce the exact trace in which
an effect-blocked finished worker releases its permit to a queued Direct
request. A missing witness is a failure because it would make the positive
claim vacuous.

The pure Rust transition model remains the semantic authority. TLC is a
separately encoded cross-check, not a second production specification. The
runner requires Java 11+ and `tla2tools.jar` through `TLA2TOOLS_JAR` or
`$HOME/.local/share/tlaplus/tla2tools.jar`; it writes TLC metadata only to a
temporary directory.

## Tools

| Command | Purpose | Writes by default |
|---|---|---|
| `python3 tx-pool/scripts/check_all.py` | Discover and run every read-only `check_*.py` contract; `--light` skips Rust test discovery and the external TLC runtime for the lightweight CI workflow. | No |
| `python3 tx-pool/scripts/check_ascii.py` | Reject non-ASCII styling in technical source, contracts and generated documentation while allowing the profiler's exact external microsecond unit token. | No |
| `python3 tx-pool/scripts/check_docs.py` | Validate links, root index coverage, script/contract documentation, retired names and CI path coverage derived from every registered implementation/workspace/integration evidence root. | No |
| `python3 tx-pool/scripts/check_formal_models.py` | Run the positive permit/effect TLC closure and the required negative reachability witness; reject setup errors, timeouts and missing exact sentinels. | Only temporary TLC metadata outside the source tree |
| `python3 tx-pool/scripts/check_production_contracts.py` | Enforce the cross-crate best-tip/startup boundary, keep reorg and generation clears on one capacity-one ordered control lane, structurally prove that each direct `AuthorityRuntime` mutation consumes one post-commit wake receipt (with only the closed mutation-free superseded-reset disposition), keep effect publication read-only and claim-bound until private settlement, keep profiling acquisition/stage/effect seams centralized and feature-gated, and keep generation invalidation behind the sole typed `AuthorityIntegrityFault` settlement boundary and closed chain error algebra. | No |
| `python3 tx-pool/scripts/check_review_guide.py` | Validate the behavior registry and generated review-guide region. | No |
| `python3 tx-pool/scripts/check_test_layout.py` | Enforce test isolation, module wiring, static panic restrictions, reviewed test-only seams, and the explicit dormant-tx-pool retirement boundary used by chain-only sync tests. | No |
| `python3 tx-pool/scripts/check_security_manifest.py` | Discover nextest tests and validate the architecture, behavior, integration and inventory contracts. | No |
| `python3 tx-pool/scripts/benchmark.py` | Produce fingerprinted Criterion records and controlled A/B comparisons. | Only requested benchmark artifacts |
| `python3 tx-pool/scripts/cross_version_benchmark.py` | Build or reuse two hash-bound one-shot binaries and produce checkpointed balanced cross-version A/B evidence with a strict noise gate. | Isolated Cargo targets and the requested external JSON artifact |
| `python3 tx-pool/scripts/profile.py capture ...` | Build or reuse one hashed profiling binary, capture a windowed Samply profile plus a separate in-memory span-count run, and emit a strict manifest and deterministic analysis. Artifacts must be outside the source tree. | Only the requested external artifact prefix |
| `python3 tx-pool/scripts/profile.py analyze --manifest ...` | Revalidate recorded artifact hashes and regenerate the window-cropped symbol summary without executing CKB code. | The summary path owned by the manifest |

## Normal review gate

```bash
python3 tx-pool/scripts/check_all.py
cargo nextest run -p ckb-tx-pool --features internal
cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings
```

The dedicated lightweight workflow
`.github/workflows/ci_tx_pool_review.yaml` compiles the scripts and runs the
deterministic documentation, behavior and layout checks. Security-manifest
validation stays with Rust CI because it performs nextest discovery.

## Intentional updates

After changing semantic behavior evidence, regenerate the guide region owned by
the registry:

```bash
python3 tx-pool/scripts/check_review_guide.py --write
```

After deliberately adding, removing or renaming tests, regenerate the exact
inventory. Counts and selectors update from discovery; they are never edited:

```bash
python3 tx-pool/scripts/check_security_manifest.py --update-inventory
python3 tx-pool/scripts/check_all.py
```

For a built integration runner, validate that the curated impact set still
exists in the executable process-test universe:

```bash
target/release/ckb-test --list-specs --bin target/release/ckb > /tmp/ckb-specs.txt
python3 tx-pool/scripts/check_security_manifest.py \
  --integration-only --integration-spec-list /tmp/ckb-specs.txt
```

`--release` additionally fails while
`security-regression-manifest.json` contains a release blocker. `--write` and
`--update-inventory` are maintainer commands and must not run in CI.

## Performance evidence

Correctness tests and their duration are not performance evidence. Follow
[`BENCHMARK.md`](BENCHMARK.md) for clean, repeated, fingerprint-matched A/B
records. A quick run is diagnostic; only the specified controlled comparison
can close the performance release condition. Follow
[`PERFORMANCE.md`](PERFORMANCE.md) for the profiling scenario contract, exact
Samply/Tokio-console/cargo-instruments commands, artifact schemas and the rule
that sampled stacks select candidates but never prove a throughput win.
