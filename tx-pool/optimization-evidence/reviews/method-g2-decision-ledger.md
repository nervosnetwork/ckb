# Tx-Pool Method g2 Decision Ledger

Role: collected self-audit input, not policy, proof, phase state or acceptance.
The owning method remains `.independent-execution-plan`. No item is final until
the cross-constraint synthesis and final-goal meta-review are complete on one
frozen subject.

Each entry separates primary fact, inference, strongest counterexample,
candidate decision and affected claim. `ADOPT-CANDIDATE` means retain for the
final synthesis; it does not mean accepted product evidence.

## Identity and scope

- Recovery ref: `refs/codex/checkpoints/txpool-proof-kernel-reset-recovery-20260814`
- Recovery commit: `bc3a2c65e604212a2493fc0e12db27313179816c`
- Recovery tree: `f54256aa52c071ffdaa9dfe1e4af7dc5424de592`
- Retired Acceptance U: `d88f2fd7e3dba27513b515fc16db72db42f30f7118644ee756242bef0b79355f`
- Frozen branch basis: current branch/worktree plus the recovery/product refs;
  live `develop`, Tor and unrelated work remain excluded.

## Collected decisions

### D01 — generic receipt false-green path

- Fact: g1 `validate_receipt` checked receipt shape, hashes, subject and named
  producer but did not validate evidence-class semantics.
- Inference: an arbitrary existing file hash could satisfy a phase receipt;
  later generic receipts could syntactically manufacture progress.
- Counterexample: a schema-valid `passed` receipt whose only evidence record is
  `AGENTS.md` satisfies every old predicate while proving no review/theorem.
- Candidate decision: review artifacts are typed process attestations only;
  machine phases fail closed unless one designated controller replays primary
  evidence. Claims become proved only after the cold joined final execution.
- Status: `ADOPT-CANDIDATE`; affects control plane and G4.

### D02 — one global authority is not one authority per fact

- Fact: root repository policy says one authority for each fact. The current
  candidate uses one physical `AuthorityStore` lock over broad tx-pool state.
- Inference: global authority count `= 1` can reward over-centralization and
  contradict maximal independent work.
- Counterexample: two operations over disjoint semantic facts commute but both
  require the same global write guard.
- Candidate decision: the floor is pointwise multiplicity one per mutable
  semantic fact; physical ownership may partition commuting facts while each
  noncommuting fact still has exactly one logical owner.
- Status: `ADOPT-CANDIDATE`; affects static coordinates 1–3 and G1.

### D03 — a connected transaction component is too coarse an atomic unit

- Fact: dependency connectivity does not by itself require an entire bounded
  component or ingress batch to become visible at one instant.
- Inference: “one Apply per connected component” may enlarge cuts and hostile
  critical-section work.
- Counterexample: sequentially admitted parent/child transactions can expose
  two legal linearizable operations even though their footprints are connected.
- Candidate decision: derive atomicity from one operation's inseparable
  write/conservation closure; connectivity informs ordering, not automatic
  whole-component atomicity.
- Status: `ADOPT-CANDIDATE`; affects D, cut floor and G1.

### D04 — current production must not be pre-certified as the witness

- Fact: the production source cut and TwoPhase bridge exist, while the broad
  physical authority guard is an unresolved hidden-serialization candidate.
- Inference: naming the current design `production_witness` before attainment
  is circular.
- Counterexample: any proven commuting pair observed crossing the same write
  guard disproves zero extra order.
- Candidate decision: record only `current_candidate_hypothesis`, retained
  achievements and strongest known attack; winner status is derived later.
- Status: `ADOPT-CANDIDATE`; affects G0/G1.

### D05 — semantic independence must be state dependent and observational

- Fact: conflict, proposal, resource and replacement outcomes depend on current
  state; implementation locks are not protocol facts.
- Inference: a static footprint-only classifier is insufficient and an
  implementation-derived classifier can certify its own serialization.
- Counterexample: two admissions commute with ample independent capacity but
  cease to commute when they compete for the last bounded global resource.
- Candidate decision: `x ||_q y` requires equality of both legal orders in
  quotient state, responses, resources, effects and in-flight observations;
  `D_q` is its complement.
- Status: `ADOPT-CANDIDATE`; affects Q/C/D and maximal parallelism.

### D06 — use a coarsest observational quotient, not another executable pool

- Fact: every correct implementation must preserve distinctions observable by
  some legal continuation; the old model copied algorithms instead.
- Inference: continuation equivalence supplies an implementation-independent
  semantic lower bound.
- Counterexample: an abstraction merging Proposed and Gap is distinguished by
  the next legal commit continuation.
- Candidate decision: define `Q = histories / continuation equivalence`; admit
  only a small symbolic quotient representation that proves both sufficiency
  (same representation implies equivalent continuations) and necessity
  (different representation has a distinguishing continuation).
- Status: `ADOPT-CANDIDATE`; the sufficiency/necessity pair must be added to the
  draft; affects Q, alpha and G0/G1.

### D07 — linearizability supplies correctness, not literal one-instruction cuts

- Fact: a completed operation must admit a legal linearization point between
  invocation and response, but implementations may use helping, retries or a
  dynamically chosen point.
- Inference: counting literal locks or machine instructions as “linearization
  points” is not invariant under implementation.
- Counterexample: a lock-free helped operation has one abstract point of effect
  without one fixed source line.
- Candidate decision: count authoritative commit/decision stages and vetoable
  cuts above the single abstract point of effect, not literal instructions.
- Status: `ADOPT-CANDIDATE`; draft wording requires refinement; affects S3/G1.

### D08 — platform scheduling is not architecture-introduced serialization

- Fact: allocator, OS scheduler, hardware coherence and external services can
  order operations without tx-pool policy doing so.
- Inference: “no implementation path” needs an owned-system boundary.
- Counterexample: two disjoint operations run sequentially on one CPU despite
  no shared tx-pool mechanism.
- Candidate decision: measure tx-pool-controlled synchronization, ownership,
  memory-conflict and queue paths; declare platform scheduling and externally
  owned resources separately, while still measuring their performance effect.
- Status: `ADOPT-CANDIDATE`; affects D/static scope/trust boundary.

### D09 — absence of statistical significance is not performance evidence

- Fact: g1 required non-inferiority but also used “opponent not statistically
  superior”, which can pass under low power.
- Inference: strongest must be established by pre-powered simultaneous
  equivalence/non-inferiority intervals, not failure to reject.
- Counterexample: a noisy underpowered run hides a material regression.
- Candidate decision: use predeclared practical margins, separate pilot, fixed
  sample count and family-wise intervals. Report both the strict empirical
  Pareto relation and the practically equivalent top class; complexity breaks
  ties only within that class.
- Status: `ADOPT-CANDIDATE`; strict-vs-equivalent wording still needs synthesis;
  affects G2/G3.

### D10 — finite empirical closure must remain explicitly finite

- Fact: no finite benchmark set proves performance against unbuilt programs.
- Inference: applying “global” to empirical performance would be false.
- Counterexample: a future implementation can be faster without contradicting
  any past measurement.
- Candidate decision: global quantification belongs to static lower bounds;
  “实测最强” quantifies over frozen built valid `C_W`, with profile-derived
  search closure and no hidden scalarization.
- Status: `ADOPT-CANDIDATE`; affects G2 wording, not goal strength.

### D11 — single deletion proves only local irredundancy

- Fact: every component may be individually necessary while two components can
  be jointly replaced by one smaller mechanism.
- Inference: old deletion certificates cannot prove global minimum complexity.
- Counterexample: remove A or B alone breaks behavior; replace `{A,B}` with C
  preserves behavior at lower burden.
- Candidate decision: require independent floors, simultaneous attainment,
  semantics-preserving exchanges and single plus interacting
  deletion/replacement attacks.
- Status: `ADOPT-CANDIDATE`; affects G3.

### D12 — “one checker” can still hide a second implementation

- Fact: file count or entrypoint count does not constrain semantic decisions
  inside one monolith.
- Inference: collapsing 30k lines into one checker could falsely reach zero.
- Counterexample: one checker reimplements proposal eligibility and calls
  itself structural.
- Candidate decision: complexity counts trusted semantic decisions and active
  roles, while LOC/AST/branch/runtime/bytes remain mandatory diagnostics and
  mutation targets. A projector may discover, hash, replay and publish but may
  not decide tx-pool semantics.
- Status: `ADOPT-CANDIDATE`; affects K/G3/control plane.

### D13 — literal globally shortest source/proof is not a defensible objective

- Fact: arbitrary program shortestness depends on language/encoding and cannot
  be established by tests or a finite architecture search.
- Inference: an unspecified “smallest code” claim is neither stable nor
  falsifiable; silently using LOC would invite minification and code golf.
- Counterexample: identical semantics can be compressed syntactically while
  increasing trusted decisions and maintenance risk.
- Candidate decision: define complexity as an explicit semantic-role product
  order with independent observation/consumer lower bounds; retain source and
  proof size diagnostics so bloat remains visible. This operationalizes rather
  than weakens the final goal.
- Status: `ADOPT-CANDIDATE`; must be explicitly defended in the meta-review;
  affects G3.

### D14 — receipts cannot defend against a writer who can edit the validator

- Fact: local repository evidence and its validator share one write authority.
- Inference: cryptographic-looking hashes cannot remove the trusted change
  boundary; claiming otherwise repeats certificate theater.
- Counterexample: a malicious writer edits both receipt and checker.
- Candidate decision: threat model prevents accidental/stale/torn/manual
  false-green paths; source changes invalidate identities and require same-
  subject review plus cold replay. Human review truthfulness and local write
  integrity remain named assumptions.
- Status: `ADOPT-CANDIDATE`; affects trust boundary/G4.

### D15 — model allocation is proposition-specific

- Fact: consensus already has a real verifier; Rust types carry local ownership;
  only interleaving/liveness and cross-operation algebra escape those tools.
- Inference: one universal executable model is the wrong authority shape.
- Counterexample: a parallel ProposalContext implementation agrees with its own
  tests while both drift from `TwoPhaseCommitVerifier`.
- Candidate decision: oracle differential for consensus, real properties for
  conservation/commutation, TLA only for the two state machines, and no model
  algorithm without a named global terminus and burden-reducing deletion test.
- Status: `ADOPT-CANDIDATE`; affects Q/C/D/mu/alpha.

### D16 — AGENTS scope check

- Fact: official Codex scope loads root-to-leaf instruction files, with closer
  files adding subtree rules. Root rules are repository-stable; tx-pool rules
  describe stable subsystem boundaries and validation defaults.
- Inference: goal phases, current status and proof method do not belong in
  `AGENTS.md`.
- Counterexample attempted: move Q/C/D or phase status into tx-pool AGENTS;
  this would apply task-specific policy to unrelated future work.
- Candidate decision: keep root AGENTS unchanged and keep tx-pool AGENTS at its
  current stable content unless a later audit finds a universal missing rule.
- Status: `ADOPT-CANDIDATE`; no further AGENTS edit currently justified.

### D17 — controller work must remain downstream of product evidence

- Fact: the final controller interface is needed to block generic receipts, but
  optimizing it before proof and measurement inputs exist can recreate the old
  tool-centric loop.
- Inference: freeze only its fail-closed role/interface now; implement the
  smallest runner when primary phase commands exist, then refreeze and rerun
  same-subject reviews.
- Counterexample: building a rich universal receipt DSL before any new Q/D
  production property exists.
- Candidate decision: the current structural projector rejects machine-phase
  artifacts without the designated controller; controller implementation is a
  later bounded slice and becomes a hashed method authority.
- Status: `ADOPT-CANDIDATE`; affects method sequence/G4.

### D18 — preflight every expensive operation

- Fact: historical mutation, benchmark and acceptance failures were sometimes
  discovered only after an expensive run had already consumed its budget.
- Inference: command validity, test discovery, tool availability, frozen
  identity, environment, disk/process isolation, timeout and a cheapest
  representative sample are separate prerequisites, not work to discover
  halfway through the expensive operation.
- Counterexample: launch a full mutation or benchmark universe before proving
  that the runner discovers the intended tests or that one sample can publish
  a complete non-torn record.
- Candidate decision: every expensive compile, mutation, formal run,
  integration universe, benchmark or Acceptance lane has a bounded preflight
  with explicit go/no-go output; preflight evidence cannot substitute for the
  full run.
- Status: `ADOPT-CANDIDATE`; candidate for stable tx-pool AGENTS scope and the
  goal-specific phase controller.

### D19 — every review needs local magnification and global propagation

- Fact: narrow reviews found deep local bugs while whole-system checklists
  found breadth gaps; either alone repeatedly allowed late discoveries.
- Inference: depth and breadth are independent coverage obligations.
- Counterexample: prove one ProposalView transition locally but omit its
  contextual verifier, reorg, template and shutdown consumers; or scan all
  files globally without tracing one transition through ownership and effects.
- Candidate decision: each review receipt names (a) one or more locally
  magnified end-to-end causal slices and (b) a global propagation scan over all
  producers, consumers, observations, failure/resource paths and same-class
  surfaces. Missing either means `partial`, never `passed`.
- Status: `ADOPT-CANDIDATE`; candidate for stable tx-pool AGENTS scope and every
  self/Partner/phase/Acceptance review schema.

### D20 — findings are discriminators, not patch specifications

- Fact: the root AGENTS already requires tracing the owning producer, consumer
  and external observation and forbids finding-shaped flags, retries, scans and
  fallbacks.
- Inference: immediately patching the line that exposed a failure preserves the
  violated design law and moves the next failure downstream.
- Counterexample: add one proposal-window exception instead of correcting the
  primitive history-to-oracle relation shared by reorg and uncle paths.
- Candidate decision: every finding first names the violated Q/C/D/mu/alpha,
  hard, performance, complexity or evidence law; searches the complete
  same-root surface; compares root corrections; and only then authorizes one
  minimum owned atomic design slice. The finding itself never dictates the
  patch shape.
- Status: `ADOPT-CANDIDATE`; root AGENTS already owns the stable rule, while the
  plan/review schema owns goal-specific enforcement.

### D21 — anti-snowball is an admission law, not final cleanup

- Fact: the rejected method stated cleanup rules but allowed each new finding
  to add model roots, registries, mappings and checkers before the cleanup gate.
- Inference: a late complexity phase cannot reliably reverse an additive daily
  development loop.
- Counterexample: add a new model classifier and census row now, promising to
  prove or delete it at X3.
- Candidate decision: before adding any state, relation, model, checker branch,
  registry, queue, task, document authority or control step, bind one unique
  claim, real production terminus, independent deletion falsifier, owner, exit
  condition and same-slice burden delta. No `B` coordinate may increase. If a
  needed discriminator requires a second authority, stop and compare cheaper
  production/oracle/property/formal alternatives instead of accruing debt.
  Independent roots may proceed in parallel; one coupled root has one in-flight
  correction.
- Status: `ADOPT-CANDIDATE`; affects daily execution, K and every phase gate.

### D22 — nondeterministic environment choices must be explicit inputs

- Fact: allocation failure, time, cancellation, endpoint results and service
  scheduling can change a tx-pool outcome independently of operation order.
- Inference: the draft equation comparing `delta_y(delta_x(q))` with the
  reverse order is ill-defined unless both runs share the same sealed external
  choices.
- Counterexample: one order sees allocation success and the other sees failure,
  falsely classifying independent operations as noncommuting.
- Candidate decision: represent environment choices as explicit labeled events
  or a paired sealed schedule. State-dependent commutativity compares the same
  environment trace and permits only the operation-order swap. This is the
  history/SIM-commutativity form; deterministic delta equality is a derived
  special case.
- Status: `ADOPT-CANDIDATE`; draft D definition requires refinement.

### D23 — independence is event-level, not whole-operation-level

- Fact: two admissions may share a final capacity/order decision while their
  resolution, script verification and planning are independent and expensive.
- Inference: classifying the whole operation as coupled permits unnecessary
  serialization before the minimum cut and violates “independent work maximal”.
- Counterexample: two transactions compete for the last resource unit, so their
  Apply results do not commute, but their sealed resolution/verification work
  can still run concurrently.
- Candidate decision: each external operation refines to typed capture,
  validate/compute, Plan, Apply and effect events. `D_q` and extra-order edges
  are defined per event/fact. Only the noncommuting mutation event enters the
  minimum authority cut; independent prefixes and suffixes remain unordered.
- Status: `ADOPT-CANDIDATE`; strictly improves the current Q/C/D/mu/alpha draft
  and affects maximal parallelism, static order and production tracing.

### D24 — the historical “light” loop was an expensive enumerator

- Fact: on an independent clean materialization of recovery commit
  `bc3a2c65e`, `check_all.py --light` ran nine discovered checkers serially and
  took `real 34.87s` (`user 33.41s`, `sys 1.25s`). It excluded the formal and
  main security-manifest validators yet still enumerated 552 model nodes, 759
  production nodes, 795 mutation rows, 593 Rust tests/2521 references, 16
  semantic axes and 62 bottom-up surfaces.
- Inference: the daily feedback loop itself amplified the closed-world model
  and made every new fact expensive to reconsider.
- Counterexample: call the route “light” because it skips two validators while
  it still runs all other semantic censuses.
- Candidate decision: the always-on projector has one bounded structural role
  and a measured sub-second target; expensive primary proof/test/measurement
  commands run only behind their preflight and owning phase. Runtime is
  diagnostic, while semantic-role removal is the real improvement.
- Status: `ADOPT-CANDIDATE`; exact historical measurement retained for method
  dominance and feedback-cost comparison.

### D25 — complexity needs semantic and engineering layers

- Fact: zero “role surplus” can still hide 41k lines inside one named role,
  while literal globally shortest source is encoding-dependent and not
  computable by this project. The immutable Chinese goal applies “全局” to
  static optimality, calls performance “实测”, and then requires minimum
  implementation/proof complexity.
- Inference: one normalized semantic vector is insufficient, but claiming a
  shortest program over all future encodings would be proof theater.
- Counterexample: merge a 30k semantic checker into one file and assign every
  branch a consumer; semantic role surplus appears zero while engineering
  burden remains enormous.
- Candidate decision: use two componentwise layers. `K_sem` proves universal
  zero semantic surplus (no duplicate authority/algorithm, trusted business
  checker decision, unterminated relation or unconsumed role). `K_eng` uses
  compiler/language/runner-derived production, test, model, proof and control
  LOC/AST/branch/dependency/artifact/build/replay observations and is minimized
  over the frozen static-bottom, performance-top-equivalent implementation set.
  A trade-off leaves no single minimum. No weights and no renaming credit.
- Status: `ADOPT-CANDIDATE`; this dominates the draft single-layer K and must be
  incorporated during synthesis; affects the completion formula and G3.

### D26 — correctness quotient and optimization costs must stay separate

- Fact: two correct implementations can expose identical protocol/public
  outcomes while using different CPU, memory, synchronization and proof effort.
- Inference: putting internal cost into continuation equivalence either prevents
  valid candidate comparison or lets the current cost shape redefine semantics.
- Counterexample: a faster allocator-free implementation and a slower allocating
  implementation have the same acceptance result but would land in different
  classes if internal allocation were part of `Q`.
- Candidate decision: `Q_H` contains only hard protocol, compatibility,
  bounded-policy disposition, committed-effect-intent and terminal observations.
  Static, empirical and engineering costs use separate sealed observers
  `S/P/K_eng`. Resource ownership and configured accept/backpressure results
  remain in `C/Q_H`; measured resource consumption does not.
- Status: `ADOPT-CANDIDATE`; draft Q/Obs wording requires refinement.

### D27 — static work floors are structural/asymptotic; constants are measured

- Fact: legal bytes, resolved edges and changed facts impose information work,
  but exact machine-level constant costs depend on representation, allocator,
  compiler and hardware.
- Inference: claiming exact zero pointwise work above an information floor can
  smuggle an unproved constant lower bound into the global theorem.
- Counterexample: two one-pass algorithms are both asymptotically optimal but
  differ by a measurable constant factor.
- Candidate decision: static work/residency observes boundedness, asymptotic
  class, duplicate passes/materializations and critical-section population
  work. Fixed-machine constants, cache effects and exact bytes/time belong to
  `P` and `K_eng`. Hard absolute ceilings remain in `H`.
- Status: `ADOPT-CANDIDATE`; affects static coordinate 5 and G1/G2 separation.

### D28 — historical prohibitions did not stop growth without an admission fuse

- Fact: checkpoint `4ef52d027` already said copying current enums into the
  model was forbidden and required each field to justify an observation,
  conservation equation, progress proof or cost bound. Later frozen history
  nevertheless reached about 1.51 MB of model source, 1.30 MB of checker source
  and 0.43 MB of selected census/certificate JSON; the plan/contract also moved
  between a 170 KB plan and a 144–252 KB contract across generations.
- Inference: good prose and file consolidation do not prevent authority growth;
  burden can migrate between artifacts while every local addition sounds
  justified.
- Counterexample: satisfy a per-field rationale, then add a registry and checker
  to track all rationales, creating two new authorities.
- Candidate decision: enforce D21 before creation with a mechanically observed
  whole-route burden delta and unique consumer, not a late narrative cleanup.
  Compare total active roles/bytes/LOC/runtime across the route, not one file.
- Status: `ADOPT-CANDIDATE`; strengthens the anti-snowball and method-dominance
  evidence.

### D29 — engineering parallelism follows the same dependence law

- Fact: read-only source/protocol/history research, isolated test shards and
  independent hypotheses can run concurrently; edits to one authority, one
  Cargo artifact graph or one coupled decision cannot safely do so.
- Inference: “use more agents/tools” is not itself maximal useful parallelism;
  the work graph needs explicit read/write and claim dependencies.
- Counterexample: two workers edit the same plan/authority or compile the same
  Cargo graph concurrently, creating merge ambiguity and slower builds.
- Candidate decision: derive an execution DAG from claims and owned artifacts;
  parallelize read-only or disjoint nodes, serialize same-authority writes and
  same-Cargo-graph compilation, and join only at a named minimal decision cut.
  Partner A remains independent and read-only.
- Status: `ADOPT-CANDIDATE`; affects fastest execution and phase scheduling.

### D30 — tools need a marginal-information admission test

- Fact: a tool can reduce feedback cost or add an independent falsifier, but
  registries and checkers can also become the work product.
- Inference: tool adoption must be judged by new claim information per active
  burden, not convenience or sophistication.
- Counterexample: add a solver that proves only natural-number nonnegativity or
  a census that restates its own inputs.
- Candidate decision: before adding a tool, name the claim, cheapest existing
  falsifier, new discriminator, primary inputs, trusted assumptions, execution
  cost, retained artifacts and retirement condition. Reject it unless it
  strictly improves falsification or replay while satisfying D21.
- Status: `ADOPT-CANDIDATE`; affects method speed and anti-snowball control.

### D31 — the hard observation alphabet is external, not implementation-shaped

- Fact: `AcceptedAtMillis` is exposed through the existing pool RPC while
  `ApplySequence` is an internal freshness/effect-order token. Both can appear
  in production state, but that alone does not give them the same semantic
  status.
- Inference: deleting every internal-looking field from `Q_H` would break
  compatibility, while retaining every stamp and diagnostic would let the
  current implementation manufacture noncommutation and certify its lock.
- Counterexample: call two admissions equivalent after changing their public
  accepted timestamps; or call them noncommuting only because swapping them
  alpha-renames two unobservable Apply sequence values.
- Candidate decision: derive the `Q_H` observation alphabet from implemented
  consensus, declared public/operational compatibility, configured bounded
  dispositions, committed effect intent and terminal behavior. Seal time and
  other nondeterministic inputs per event. An internal identity, stamp, metric
  or cache fact is retained only when a legal continuation distinguishes it;
  otherwise it is quotient-renamable. Public timestamp compatibility remains
  hard even though timestamp does not own ordering policy.
- Status: `ADOPT-CANDIDATE`; refines D26 and prevents both hidden degradation
  and implementation-induced coupling.

### D32 — use a state-dependent event partial order, not a second state machine

- Fact: production admissions already have typed capture, compute, Plan, Apply
  and effect boundaries. Lipton reduction explains when movable actions may be
  reasoned about outside an indivisible region, and the Scalable Commutativity
  Rule links state-dependent interface commutativity to conflict-free
  implementations. Transaction chopping gains concurrency only under a known
  finite transaction set, which is not the open tx-pool domain.
- Inference: the useful mathematical object is the causal/conflict order among
  production events and semantic facts, not another executable lifecycle.
  Lipton/chopping conditions are construction heuristics; they do not by
  themselves prove CKB history equivalence, resources or effects.
- Counterexample: adopt a static transaction conflict graph and claim global
  completeness even though future transactions, reorg events and
  state-dependent capacity outcomes were absent from its closed set.
- Candidate decision: refine each operation into typed events with sealed
  environment labels and named semantic read/write facts. Order edges are only
  real time, causal data dependence and `Q_H`/`C` noncommutation; the required
  order is their transitive closure. Move independent prefixes/suffixes outside
  the minimum authoritative commit/veto cut, but discharge correctness by
  history-level `Q_H` equivalence, conservation and linearizability. Do not
  instantiate a general event-structure, trace-theory or transaction-chopping
  engine unless a later claim cannot be falsified more cheaply.
- Status: `ADOPT-CANDIDATE`; replaces the operation-level `D` draft with the
  smallest event/fact skeleton and explicitly rejects three snowball routes.

### D33 — anti-snowball distinguishes authority from disposable experiments

- Fact: requiring every exploratory product edit to decrease every source
  metric would prevent causal experiments, while allowing an experiment to
  enter the active proof/control route creates the historical additive loop.
- Inference: the admission law must be strict for authoritative proof/control
  machinery and time-bounded for disposable candidate experiments.
- Counterexample: keep three losing architecture variants and their bespoke
  checkers because each once tested a plausible performance seam; or reject a
  small temporary product variant that is needed to falsify a dominant cost
  hypothesis.
- Candidate decision: any authoritative proof/control addition must replace
  its predecessor in one slice with componentwise nonincreasing active burden.
  A product experiment may temporarily increase `K_eng` only when it tests a
  named earlier-objective prediction; it is isolated, non-authoritative,
  content-addressed, size/time bounded and has an expiry. Losers and their
  exclusive artifacts retire before another coupled root starts. Promotion to
  production requires the full hard/static/performance comparison and then
  enters complexity minimization.
- Status: `ADOPT-CANDIDATE`; makes D21 executable without forbidding useful
  experimentation.

### D34 — preflight is a bounded abort/replan gate, never proof evidence

- Fact: historical full checks, Cargo builds, model enumerations, mutation
  runs, formal searches and benchmarks can consume minutes to hours and can be
  invalidated by a wrong source identity, missing tool, insufficient disk,
  undiscovered test or competing build.
- Inference: an expensive run should start only after cheap checks establish
  that its command can answer the named claim on the intended subject.
- Counterexample: run a full benchmark before confirming that both binaries
  were built from the frozen trees; or treat successful package discovery as
  performance evidence.
- Candidate decision: preflight records the claim, exact subject, command/tool
  identity, discovered universe, resource/isolation availability, expected
  discriminator, abort conditions and output destination. It may abort or
  replan; it never satisfies the primary gate. Cache a preflight only while all
  of those identities remain unchanged.
- Status: `ADOPT-CANDIDATE`; sharpens D18 and applies before every expensive
  operation.

### D35 — review verdicts are collected, synthesized once and meta-reviewed

- Fact: serially patching each review finding makes the architecture follow
  discovery order, hides shared roots and repeatedly invalidates downstream
  work.
- Inference: findings must remain independent discriminators until their
  cross-constraints and common causes are visible.
- Counterexample: one local lock finding creates sharding, the next resource
  finding adds a reservation service, and the third effect-order finding adds
  a coordinator, although one fact-partition design could resolve all three.
- Candidate decision: each review generation has a bounded collection window;
  observations enter one neutral ledger with fact, inference, counterexample
  and status. After collection closes, perform one cross-constraint synthesis,
  reject dominated combinations, and rerun the immutable final goal from a
  higher level. Only that meta-review may authorize atomic implementation
  slices. Partner A follows the same rule and remains read-only.
- Status: `ADOPT-CANDIDATE`; directly governs self-review, Partner review and
  every later phase review.

### D36 — independent batching is amortization, not independent Apply concurrency

- Fact: current production classifies a bounded Ready cohort, prepares a
  shared `ProjectionDelta` and resource plan, and commits independent members
  in one `SettlementPlan::IndependentRun`. The runtime still executes that
  Apply while holding the single `AuthorityStore` write guard and reserves one
  Apply sequence for the batch.
- Inference: this is real removal of repeated planning/publication work, but it
  does not by itself attain the goal's maximum independent concurrency. Nor
  does the existence of one guard prove that independently committable fact
  partitions exist.
- Counterexample: advertise a batch of ten independent transactions as ten
  concurrent Apply transitions; or shard the store before exhibiting a legal
  event pair whose `Q_H`/`C` observations commute under the same environment.
- Candidate decision: preserve the batch as a positive production asset.
  Separately use adjacent-swap production traces and lock/critical-section
  probes to establish at least one event-level commuting Apply pair before
  treating the global guard as a static-attainment failure. Compare batching,
  partitioned Apply and reservation alternatives under the same hard facts,
  structural work bounds and fixed-binary performance method.
- Status: `ADOPT-CANDIDATE`; keeps the current candidate explicitly unproved
  and prevents both false condemnation and false certification.

## Open synthesis questions

1. Can the symbolic representation of `Q` prove both sufficiency and necessity
   without becoming another executable tx-pool?
2. Is the static coordinate for one abstract point of effect best represented
   as authoritative commit stages, veto points, or another invariant measure?
3. What exact owned-system boundary distinguishes tx-pool-added order from OS,
   allocator and unavoidable global resource coordination?
4. Should “measured strongest” require strict Pareto dominance, or the unique
   practically equivalent top class plus complexity minimization? The choice
   must preserve the user's no-degradation intent and remain statistically
   testable.
5. Can every complexity floor be tied to an external consumer or
   distinguishing continuation, or must an unprovable coordinate remain open?

## Required final synthesis

The synthesis must produce one coherent method, not a union of all proposals.
It must explicitly reject dominated alternatives, check cross-coordinate
interactions, rerun the final goal from first principles, list all still-open
product claims, and only then authorize a frozen same-subject self-review.
