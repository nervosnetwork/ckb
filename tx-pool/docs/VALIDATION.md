# Tx-Pool Validation and Machine Contracts

The final objective is single-owned by
[`architecture-contract.json#/optimization_goal`](../architecture-contract.json#/optimization_goal).
Validation checks that every current tx-pool document references that object,
that its hard constraints and `X0 -> X1 -> X2 -> X3` ordering cannot drift, and
that an optimality claim remains `unproved` until the static, empirical,
complexity and production-refinement certificate is complete. This document
owns maintenance procedure, not another copy of the target.

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
| [`architecture-contract.json`](../architecture-contract.json) | Stable UAK vocabulary, T1-T16 obligations, the selected bounded semantic exchange, ordered implementation slices and their exact costs/falsifiers, executable mathematical proof policy, finite convergence/evidence DAG, scoped global-optimality certificate protocol, semantic model/production roots, mutation families, non-authoritative historical regression samples, release surfaces and residual risks. Validators derive invariant vocabulary, candidate partitions and test coverage rather than copying them. | `check_security_manifest.py`, `check_model_refinement.py`, `check_mutation_adjudication.py` | Edit only for an architecture decision; update `ARCHITECTURE.md`, `PERFORMANCE.md`, behavior evidence and tests together. A blueprint slice must retain its current source anchors; an implemented slice must expose its target owner. Historical samples may seed a falsifier but never a current phase, implementation or landing decision. Never copy Rust enums, public methods, candidate rows, test names, counts or a generated current-code observation into a validator. |
| [`optimization-evidence/`](../optimization-evidence) | Content-addressed Construction and certificate projections. The semantic-refinement census independently joins normative axes, bottom-up boundaries and every discovered model-input producer while preserving open relations as owned gaps. The normal-form artifact regenerates feasibility and singleton `X1 => X2` selection. The complexity artifact derives absolute `Kappa` from boundary families, model/production roles, compile roles and proof policy; binds the state-bearing necessity census and production witness; and carries independent negative canaries. | `check_semantic_refinement_census.py`, `check_security_manifest.py` | Change a semantic axis or input producer only with a reviewed model decision. The semantic census is source-hash bound and never supplies release evidence while a successor gap remains. `--write-optimization-evidence` is admitted only while the roadmap's architecture-synthesis or production-certificate owner phase is active; it binds only that phase's requirements and validates the complete transition before writing. Stored contents must equal live generated projections, not merely their SHA-256. |
| `optimization-evidence/acceptance-universe.json` | Generated immutable `U`: exactly the seven contract-declared cuts for the current-branch X3 production subject, resolved features, test inventory, configuration/migration, tool semantics, optimization certificate and workload/environment. It binds the exact X3 checkpoint tree plus every recursively reachable submodule `(path, commit, tree, tree-listing hash)` while keeping proof tools in their independent evidence-DAG node, so a tool-only rebind cannot fabricate a product change. | `check_security_manifest.py` | Generate only with `--freeze-acceptance-universe` from the exact zero-rank post-simplification X3 state. Every required tool source, including ignored instruction files, must exist byte-for-byte in the checkpoint; its embedded manifest/progress/AGENTS projection must be internally consistent before freeze. Every frozen gitlink object must already be locally recoverable; missing, extra, stale, self-referential or phase-premature categories are rejected. Never edit or reserialize by hand. |
| [`review-behaviors.json`](../review-behaviors.json) | Stable `TP-*` rule/attack semantics, exact production-symbol owners, T1-T16 mappings and curated unit/integration evidence. Cross-crate counterexamples are typed separately, bind mechanically to an `OPEN` finding, and do not count as conformance. Focused commands and the review table are generated from it. | `check_review_guide.py`, `check_security_manifest.py`, `check_test_layout.py` | Edit only when behavior, ownership or proof evidence changes. Every symbol and exact test must resolve; no command or count field is allowed. |
| [`integration-impact.json`](../integration-impact.json) | Curated complete set of registered process specs whose production paths cross tx-pool ingress, verification, pool mutation, relay, mining/template or transaction-bearing reorg boundaries. | `check_review_guide.py`, `check_security_manifest.py` | Hand-edit when a relevant spec is added, removed, renamed or its boundary changes. Integration CI checks it against `ckb-test --list-specs`. |
| [`mutation-acceptance-lock.json`](../mutation-acceptance-lock.json) | Generated V1 projection binding the complete discovered cargo-mutants universe to one semantic obligation per row, generated invariant/falsifier evidence and the complete library Nextest universe. It retains cargo-mutants' structured replacement for semantic proof selection while excluding source diffs, spans and logs. Its input hash vector types it as current or historical; a historical lock remains typed read-only evidence while the independent mutation lane is pending, but can neither execute nor close that lane and becomes invalid once `complete_mutation` is published. | `check_mutation_matrix.py` | Rediscover only with `--rediscover --write-lock` from a clean reviewed semantic checkpoint. Never edit or rebind rows, paths, functions, replacements, tests, counts, commands or digests by hand. Current drift prevents execution/acceptance; aggregate correctness may inspect the recoverable historical envelope without consuming its source coordinates or serializing behind mutation. |
| [`mutation-result-lock.json`](../mutation-result-lock.json) | Generated portable result projection binding the candidate lock and exact raw-outcome digests to one reconciled outcome per unique candidate. It preserves the raw tool result and derives exactly one `caught`, `compile_unviable`, `equivalent(proof_id)` or `unaccepted` disposition. Architecture-owned proof selectors and behavior evidence may accept only a missed mutant under an exact executable producer/transition theorem; timeout and unstarted work remain blockers. | `check_mutation_matrix.py`, `check_mutation_adjudication.py` | Generate only with one or more `--verify-outcomes` inputs plus `--write-result-lock`. Never edit candidate identities, outcomes, dispositions, proof IDs, counts or artifact digests by hand. `accepted: false` remains a diagnostic checkpoint rather than V1 evidence. Adding proof tests intentionally makes its candidate lock historical; a new active lock and execution are required before V1 acceptance. |
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
         +----------------------------> non-authoritative historical falsifier catalog
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

## Formal residual falsifiers

[`formal/`](../formal/) contains only independently encoded residual claims
for which schedule exploration adds evidence beyond the executable Rust
model. [`models.json`](../formal/models.json) is the single machine-readable
run registry; the checker discovers every `.tla` and `.cfg` file and rejects
missing, duplicate or stale registrations. TLC model count and state count are
not coverage objectives: expanding a module to duplicate the Rust protocol
would create a second specification and increase proof complexity.

[`PermitEffect.tla`](../formal/PermitEffect.tla) checks multi-capacity fair
permit handoff, bounded worker occupancy, effect pressure and publisher
progress. [`PermitEffect.cfg`](../formal/PermitEffect.cfg) closes its complete
finite state space. [`PermitEffectReachability.cfg`](../formal/PermitEffectReachability.cfg)
deliberately checks a false invariant and must expose the trace in which an
effect-blocked finished worker releases its permit to a queued Direct request.

[`ProposalLiveness.tla`](../formal/ProposalLiveness.tla) independently checks
the residual cross-block claim for a finite stable eligible cohort: commits
come only from the inclusive proposal window and positive fair service drives
the uncommitted rank to zero. [`ProposalLiveness.cfg`](../formal/ProposalLiveness.cfg)
must close with safety and eventual commitment. The outage configuration
[`ProposalLivenessReproposal.cfg`](../formal/ProposalLivenessReproposal.cfg)
checks a deliberately false invariant and must expose expiry followed by
re-proposal, preventing a vacuous liveness proof that never reaches the
re-proposal frontier.

The pure Rust transition model remains the sole semantic authority; it owns
proposal-history derivation, causal selection and production differential
relations. TLC is a separately encoded bounded falsifier, not a second
production specification. The runner requires Java 11+ and `tla2tools.jar`
through `TLA2TOOLS_JAR` or `$HOME/.local/share/tlaplus/tla2tools.jar`; it writes
TLC metadata only to a temporary directory.

## Tools

| Command | Purpose | Writes by default |
|---|---|---|
| `python3 tx-pool/scripts/check_all.py` | Discover and run every read-only `check_*.py` contract; `--light` skips Rust test discovery and the external TLC runtime for the lightweight CI workflow. | No |
| `python3 tx-pool/scripts/check_ascii.py` | Reject non-ASCII styling in technical source, contracts and generated documentation while allowing the profiler's exact external microsecond unit token. | No |
| `python3 tx-pool/scripts/check_docs.py` | Validate links, root index coverage, script/contract documentation, retired names and CI path coverage derived from every registered implementation/workspace/integration evidence root. | No |
| `python3 tx-pool/scripts/check_formal_models.py` | Discover every registered TLA module/config pair, reject registry drift, run each expected verdict, and require the named negative reachability witness. | Only temporary TLC metadata outside the source tree |
| `python3 tx-pool/scripts/check_mutation_adjudication.py` | Validate current proof-family schemas and exact generated evidence, then classify the lock from its complete input hash vector. Current locks require exact candidate/source-site resolution. Historical Construction locks retain only a byte-recoverable, internally closed root-adjudication record and never supply current coordinates. `--write-evidence` synchronizes only mechanically selected evidence rows in `review-behaviors.json`. | No by default; only `--write-evidence` updates the generated evidence rows |
| `python3 tx-pool/scripts/check_mutation_matrix.py` | Fast-default validation of a current complete lock/result or a typed read-only historical Construction pair. Explicit `--rediscover` joins semantic owners to structured cargo-mutants discovery; `--write-config` subtracts prior outcomes, and repeated `--verify-outcomes` inputs assemble one identity-closed result while rejecting inconsistent tool-required replays. The generated command runs four independent mutants concurrently under cargo-mutants' shared machine jobserver; this preserves the exact candidate/test universe while bounding aggregate Cargo concurrency. Operational flags reject historical locks; Acceptance/Accepted additionally require a current accepted result. | Only explicitly requested locks/configs; default validation is read-only and does not invoke cargo-mutants |
| `python3 tx-pool/scripts/check_model_refinement.py` | Derive Cargo/module source roles, then build the directional state-bearing ownership graph from architecture semantic roots, behavior-registry production entrypoints and registered model/test evidence. Type dependencies, impl ownership, free-function calls and function values are connector edges; aliases, constants, macros and traits remain named syntax boundaries rather than a falsely complete function census. The default gate rejects any model or production enum/struct outside the graph. Permanent canaries require registered free-function/evidence reachability while keeping unregistered evidence and disconnected capabilities outside. `--json` emits the complete read-only state-bearing frontier; no generated observation is an allowlist. | No |
| `python3 tx-pool/scripts/check_model_refinement.py --variant-flow --cargo-expand-production` | Run the slower M3 route gate with `ast-grep` and current `cargo expand` output. Before classification it rejects every ast-grep syntax-error tree and removes only rustc's lint-only `non_exhaustive_omitted_patterns` marker, which the compiler pretty-printer places in expression positions that tree-sitter cannot round-trip. It distinguishes source producers/consumers from macro-expanded evidence, rejects a rooted model or expanded-production enum variant with no producer, and requires an explicit construction witness for every registered model and expanded-production root struct, including task owners and move-only capabilities. Expanded derive matches never replace source-level consumer evidence. | Only Cargo build cache and temporary expansion outside the source tree |
| `python3 tx-pool/scripts/check_production_contracts.py` | Enforce a closed Rust module graph with no uncompiled source residue; enforce the cross-crate best-tip/startup boundary; keep reorg and generation clears on one capacity-one ordered control lane; structurally prove that each direct `AuthorityRuntime` mutation consumes one post-commit wake receipt (with only the closed mutation-free superseded-reset disposition); keep all Apply arms between one before/after wake cut and bind its six observations to exact Notify edges; bind Ready OCC to the scheduler's allocation-free, hash/version-exact longest common strict-priority prefix, require stale scratch retirement after the read guard, and require one cancellation-aware cooperative handoff after each bounded progress attempt; bind direct-negative OCC to the transaction-bounded producer/spender owner-version read relation, reject a global Accepted clock, require coherent refresh plus lock-external retirement, and exercise missing-spender/currentness negative canaries; keep effect publication read-only, claim-bound and one log-owned `Receipt | Idle | ClosedAndDrained` observation until private settlement; keep replacement and administrative release on one projected-final-owner law whose resolver and membership producers seal strict pool-output references; keep every owner change in the closed `Insert | Replace | Remove` relation and bind membership retirement policy to its fallibly preallocated Plan-to-Apply carrier; bind both expiry planners to their sole bounded due-index producers; bind admission evidence to sealed constructors, construct worker capability and `ActiveWork` from one owner cut, retain deferred settlement as one move-only carrier, and classify the common-baseline settlement through a closed declared-cycle policy evolution; keep retained proposal ingress batched; keep profiling acquisition/stage/effect seams centralized and feature-gated; keep status RPC wiring outside optional detail arithmetic; derive exact model/production ordinary, retained-ingress, recovery and structural fault bijections; keep generation invalidation behind one authority-private fault algebra carried across service and ordered-chain boundaries only as the move-only `AuthorityGenerationInvalidity` capability; derive the complete default task/channel owner and join-cut census; require invalid retirement to retain and join every aborted handle; prove that sync and RPC cross-crate fixtures join tx-pool before relay/database/chain/runtime teardown and bind loopback clients against ambient proxies; reject detached tasks, proxy-intercepted fixture endpoints, fixture drop-order reversal and abort-without-join through executable negative canaries; and keep notification endpoint futures inside one joined topology lane. | No |
| `python3 tx-pool/scripts/check_review_guide.py` | Validate the behavior registry and generated review-guide region. | No |
| `python3 tx-pool/scripts/check_semantic_refinement_census.py` | Prove equality of independently sourced normative, implementation and relation axes; bind every bottom-up boundary to one owner; enumerate every discovered model input carrier and reject free derived proposal states, incomplete completion identity and unproved cost-summary congruence. It validates source/content hashes and negative canaries but does not turn an owned open relation into release evidence. | No |
| `python3 tx-pool/scripts/check_test_layout.py` | Enforce test isolation, module wiring, static panic restrictions, reviewed test-only seams, and the explicit dormant-tx-pool retirement boundary used by chain-only sync tests. | No |
| `python3 tx-pool/scripts/check_security_manifest.py` | Discover Nextest tests; validate architecture, behavior, integration and inventory contracts; enforce both convergence ranks/evidence DAGs, blocker ownership, invalidation classes and terminal mutation outcomes; and reject an uncertified, phase-premature or downstream-only global-optimality claim. `--write-optimization-evidence` regenerates phase-owned Construction projections. `--rebind-optimization-tool-evidence` requires byte-equal release basis/workload and equality after erasing only proof identity; product identity is an independent evidence-DAG node. `--freeze-acceptance-universe` derives the exact seven-category current-branch X3 `U`. `--reopen-acceptance-evidence` atomically retires the current U and all result descendants after a proof-tool change, even after partial Acceptance progress, while preserving the independently certified product. `--execute-acceptance-phase PHASE --acceptance-output FILE` is the sole rank-bearing lane entry: it calls the U-bound runner, derives observations from its owned artifacts, cold-validates the exact prospective joined state in a fresh Git root, and publishes result, artifacts, progress and manifest as one rollback-safe transaction. Direct `--publish-acceptance-result` is rejected. | No by default; explicit write modes update only their owned generated artifacts and manifest projection |
| `python3 tx-pool/scripts/run_acceptance_lane.py --phase PHASE --output FILE` | Execute one fixed existing Acceptance lane and emit a diagnostic result bound to the frozen U, X3 checkpoint/tree, recursive submodule source closure, runner identity, exact command plan, environment and output hashes. It materializes exact gitlinks only from already verified local frozen object stores; network availability is not a correctness premise. A separately produced JSON owns no phase authority; only the canonical execute-and-publish entry above can lower rank. | Cargo/integration build outputs and only the requested diagnostic result file |
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
deterministic documentation, behavior and layout checks. Security-manifest
validation stays with Rust CI because it performs nextest discovery. External
branch reconciliation is deliberately absent from this current-branch goal.

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
identity. If any bound input changes after execution, the lock becomes
`historical_non_release`. Construction validates its immutable envelope and
keeps the aggregate usable, but execution flags reject it and current candidate
resolution is deferred. Pending Acceptance may inspect the same historical
envelope without consuming it; a completed mutation lane and Accepted reject it
outright. Generate and
execute a fresh current lock after the reviewed Construction checkpoint; never
rebind historical outcomes to the new universe.

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

`Unviable` means the generated Rust failed to compile and is not a survivor. A
`MissedMutant` is accepted only when its current structured candidate matches
exactly one architecture-owned proof ID whose executable producer/transition
evidence is present in the generated behavior graph. Timeout, unexpected,
ambiguous, zero-proof, stale-proof or unstarted work blocks V1. Mutation
evidence remains a falsifier of the registered laws, never a second
specification or permission to patch code for the tool.

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
