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
| [`architecture-contract.json`](../architecture-contract.json) | Stable UAK vocabulary, T1-T16 obligations, the selected bounded semantic exchange, ordered implementation slices and their exact costs/falsifiers, executable mathematical proof policy, semantic roots for the generated model/production refinement frontier, the semantic V1 mutation-adjudication families, immutable develop comparison cases, release-surface anchors and durable residual risks. Validators derive invariant vocabulary, exact candidate partitions and test coverage rather than copying them. | `check_security_manifest.py`, `check_model_refinement.py`, `check_develop_refinement.py`, `check_mutation_adjudication.py` | Edit only for an architecture decision; update `ARCHITECTURE.md`, `PERFORMANCE.md`, behavior evidence and tests together. A blueprint slice must retain its current source anchors; an implemented slice must expose its target owner. Never copy Rust enums, public methods, candidate rows, test names, counts or a generated current-code observation into a validator. |
| [`review-behaviors.json`](../review-behaviors.json) | Stable `TP-*` rule/attack semantics, exact production-symbol owners, T1-T16 mappings and curated unit/integration evidence. Cross-crate counterexamples are typed separately, bind mechanically to an `OPEN` finding, and do not count as conformance. Focused commands and the review table are generated from it. | `check_review_guide.py`, `check_security_manifest.py`, `check_test_layout.py` | Edit only when behavior, ownership or proof evidence changes. Every symbol and exact test must resolve; no command or count field is allowed. |
| [`integration-impact.json`](../integration-impact.json) | Curated complete set of registered process specs whose production paths cross tx-pool ingress, verification, pool mutation, relay, mining/template or transaction-bearing reorg boundaries. | `check_review_guide.py`, `check_security_manifest.py` | Hand-edit when a relevant spec is added, removed, renamed or its boundary changes. Integration CI checks it against `ckb-test --list-specs`. |
| [`mutation-acceptance-lock.json`](../mutation-acceptance-lock.json) | Generated V1 projection binding the complete discovered cargo-mutants universe to one semantic obligation per row, generated invariant/falsifier evidence and the complete library Nextest universe. It freezes input/tool hashes and exact candidate/config/command digests, so the aggregate gate can validate it without requiring cargo-mutants. | `check_mutation_matrix.py` | Rediscover only with `--rediscover --write-lock` from a clean reviewed semantic checkpoint. Never edit rows, paths, functions, tests, counts, commands or digests by hand; the default fast check rejects drift. |
| [`mutation-result-lock.json`](../mutation-result-lock.json) | Generated portable result projection binding the candidate lock and exact raw-outcome digests to one reconciled outcome per unique candidate. It records execution and replay counts and rejects a repeated identity whose outcome changes across runs. The architecture-owned adjudication families mechanically partition every missed/timed-out row without changing the historical outcome. | `check_mutation_matrix.py`, `check_mutation_adjudication.py` | Generate only with one or more `--verify-outcomes` inputs plus `--write-result-lock`. Never edit candidate identities, outcomes, counts or artifact digests by hand. `accepted: false` remains a diagnostic checkpoint rather than V1 evidence. Adding proof tests intentionally makes its candidate lock historical; a new active lock and execution are required before V1 acceptance. |
| [`security-regression-manifest.json`](../security-regression-manifest.json) | Assembly manifest binding package/features, architecture, behavior, integration universe, generated inventory, V1 semantic mutation obligations and explicit release blockers. Mutation obligations reference existing topology components, semantic bindings and behavior owners; they never copy candidate rows, paths, test names, commands, counts or digests. | `check_security_manifest.py`, `check_mutation_matrix.py` | Edit only when an assembly input, semantic mutation scope or release decision changes. Candidate rows, paths, tests, commands, counts and digests are always derived. |
| [`test-layout-manifest.json`](../test-layout-manifest.json) | Allowed dedicated test roots plus named irreducible test observation seams. Module wiring and `cfg(test)` sites are discovered from Rust rather than copied. | `check_test_layout.py` | Edit only for a deliberate directory boundary or exceptional seam. A current observation is never accepted merely by copying it into an allowlist. |
| [`test-inventory.txt`](../test-inventory.txt) | Exact sorted snapshot of discovered internal-feature Rust tests and the managed integration spec names. This is generated evidence, not a hand-written selection list. | `check_security_manifest.py` | Regenerate with `--update-inventory` after an intentional test/integration inventory change, then review the diff. |

The evidence direction is one-way:

```text
Rust source -------- discovers -------> owner/state/identity facts
nextest + ckb-test -- discovers -------> complete executable test universe
         |
         v exact reconciliation
architecture-contract.json -----------> T1-T16 vocabulary + selected topology/slices
         +----------------------------> immutable develop source/counterexample cases
review-behaviors.json -----------------> semantic rules + exact owners/evidence
integration-impact.json --------------> curated process boundary
         |
         +-- generates ----------------> REVIEW_GUIDE.md commands and tables
         +-- generates ----------------> test-inventory.txt
         +-- validates ----------------> test-layout exceptions and CI gates
security-regression-manifest.json ----> semantic mutation obligations
         + cargo-mutants JSON --------> complete row lock + exact run config
         + outcome JSON partitions ---> portable result projection
architecture mutation families
         + candidate/result locks ----> exact root-proof partition
         + discovered test inventory -> generated behavior evidence
```

The only manual layer is semantic: why a rule exists, its attack case,
compatibility and performance bounds, and whether a residual risk is accepted.
Everything mechanically derivable has one owner and is generated or checked.
CI is read-only and fails on dangling symbols, zero-match evidence, missing
T1-T16 coverage, unregistered relevant process specs, stale inventory,
test-code leakage or generated Markdown drift.

## Formal falsifier

The bounded permit/effect protocol has an independent TLA+ falsifier under
[`formal/`](../formal/). [`models.json`](../formal/models.json) is the single
machine-readable run registry; the checker discovers every `.tla` and `.cfg`
file and rejects missing, duplicate or stale registrations.
[`PermitEffect.tla`](../formal/PermitEffect.tla) models multi-capacity fair
permit handoff, bounded worker occupancy, effect pressure and publisher
progress. [`PermitEffect.cfg`](../formal/PermitEffect.cfg) must close the
complete finite state space with its safety and liveness properties.
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
| `python3 tx-pool/scripts/check_develop_refinement.py` | Require the full immutable develop baseline commit/tree, extract every registered historical function directly from Git, verify the semantic call-order facts, cover F1-F8 and bind each negative witness to current behavior evidence. | No |
| `python3 tx-pool/scripts/check_docs.py` | Validate links, root index coverage, script/contract documentation, retired names and CI path coverage derived from every registered implementation/workspace/integration evidence root. | No |
| `python3 tx-pool/scripts/check_formal_models.py` | Discover every registered TLA module/config pair, reject registry drift, run each expected verdict, and require the named negative reachability witness. | Only temporary TLC metadata outside the source tree |
| `python3 tx-pool/scripts/check_mutation_adjudication.py` | Join the checked-in candidate/result locks to architecture-owned anchored function selectors, require every missed/timed-out identity to have exactly one root-proof family, resolve its model/production roles and exact evidence from the generated test inventory, and reject copied candidate rows or counts. `--write-evidence` synchronizes only the mechanically selected evidence rows in `review-behaviors.json`. | No by default; only `--write-evidence` updates the generated evidence rows |
| `python3 tx-pool/scripts/check_mutation_matrix.py` | Fast-default aggregate validation of the checked-in complete candidate lock and optional result projection. Explicit `--rediscover` joins semantic owners to structured cargo-mutants discovery; `--write-config` subtracts prior outcomes, and repeated `--verify-outcomes` inputs assemble one identity-closed result while rejecting inconsistent tool-required replays. | Only explicitly requested locks/configs; default validation is read-only and does not invoke cargo-mutants |
| `python3 tx-pool/scripts/check_model_refinement.py` | Starting only from semantic roots in `architecture-contract.json`, derive the reachable model and production Rust types, enum members, implementation methods, reference counts and synchronization primitives. Every run executes permanent parser, deliberately-unbound semantic-root and deliberately-unconstructed-capability canaries in `scripts/fixtures/model_refinement_canary.rs`. `--json` emits the complete read-only M3 frontier; no generated observation is an allowlist. | No |
| `python3 tx-pool/scripts/check_model_refinement.py --variant-flow --cargo-expand-production` | Run the slower M3 route gate with `ast-grep` and current `cargo expand` output. It distinguishes source producers/consumers from macro-expanded evidence, rejects a rooted model or expanded-production enum variant with no producer, and requires an explicit construction witness for every registered model and expanded-production root struct, including task owners and move-only capabilities. Expanded derive matches never replace source-level consumer evidence. | Only Cargo build cache and temporary expansion outside the source tree |
| `python3 tx-pool/scripts/check_production_contracts.py` | Enforce a closed Rust module graph with no uncompiled source residue; enforce the cross-crate best-tip/startup boundary; keep reorg and generation clears on one capacity-one ordered control lane; structurally prove that each direct `AuthorityRuntime` mutation consumes one post-commit wake receipt (with only the closed mutation-free superseded-reset disposition); keep retained proposal ingress batched; keep effect publication read-only and claim-bound until private settlement; keep profiling acquisition/stage/effect seams centralized and feature-gated; keep status RPC wiring outside optional detail arithmetic; and keep generation invalidation behind the sole typed `AuthorityIntegrityFault` settlement boundary and closed chain error algebra. | No |
| `python3 tx-pool/scripts/check_review_guide.py` | Validate the behavior registry and generated review-guide region. | No |
| `python3 tx-pool/scripts/check_test_layout.py` | Enforce test isolation, module wiring, static panic restrictions, reviewed test-only seams, and the explicit dormant-tx-pool retirement boundary used by chain-only sync tests. | No |
| `python3 tx-pool/scripts/check_security_manifest.py` | Discover nextest tests and validate the architecture, behavior, integration and inventory contracts. | No |
| `python3 tx-pool/scripts/run_managed_integration.py [--anchors]` | Derive the complete managed impact set, or the focused security anchors, directly from the canonical JSON authorities and invoke `make integration` with concurrency one. `--dry-run` validates and prints the derived invocation without building or running tests. | Release binaries and process-test artifacts through `make integration` |
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

All Rust test execution, including focused regressions, uses `cargo nextest
run`. Direct `cargo test` execution is not accepted as tx-pool evidence because
it does not provide the required per-test process isolation.

Run process-level acceptance without copying spec names from generated prose:

```bash
python3 tx-pool/scripts/run_managed_integration.py
```

The runner derives its arguments from `integration-impact.json` and
`review-behaviors.json`, validates that both name the same `make integration`
boundary, and refuses duplicate, malformed or out-of-universe anchors.

The dedicated lightweight workflow
`.github/workflows/ci_tx_pool_review.yaml` compiles the scripts and runs the
deterministic documentation, behavior, develop-refinement and layout checks.
Its checkout uses complete Git history because the develop gate refuses an
unverifiable shallow baseline. Security-manifest
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
python3 tx-pool/scripts/check_mutation_adjudication.py --write-evidence
python3 tx-pool/scripts/check_review_guide.py --write
python3 tx-pool/scripts/check_all.py
```

The adjudication write derives names from anchored family patterns and the
generated inventory. It never changes a mutation outcome or copies a candidate
identity. If the test universe or semantic registry changed after an executed
mutation lock, `check_mutation_matrix.py` must stay red until a reviewed clean
checkpoint generates and executes a new active lock; do not rebind historical
outcomes to the new universe.

For the V1 mutation checkpoint, first commit the reviewed semantic obligations.
Then generate the row-level lock from that clean input revision and review the
diff; never edit a row, count, digest or regular expression by hand:

```bash
python3 tx-pool/scripts/check_mutation_matrix.py --rediscover --write-lock
python3 tx-pool/scripts/check_mutation_matrix.py \
  --write-config /private/tmp/ckb-tx-pool-v1-mutants.toml
```

The generated lock owns the complete candidate and test-universe digests. The
temporary config contains only mechanically derived anchored expressions.
`--resume-outcomes` subtracts exact prior results. Cargo-mutants may still
regenerate an inseparable structural class; relisting must prove that every
extra row is an already completed replay, and result assembly rejects any
changed replay outcome. No row is copied by hand. Execute the printed command
only from the frozen clean checkpoint, replacing `<CONFIG>` and `<OUTPUT>`
with those exact external paths. After all runs complete, merge them and write
the portable result projection:

```bash
python3 tx-pool/scripts/check_mutation_matrix.py \
  --verify-outcomes /private/tmp/ckb-tx-pool-v1-mutants \
  --write-result-lock
```

`Unviable` means the generated Rust failed to compile and is not a survivor;
every `MissedMutant`, timeout, unexpected row or unstarted row blocks V1 and
requires semantic adjudication. Mutation evidence remains a falsifier of the
registered laws, never a second specification or permission to patch code for
the tool.

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
