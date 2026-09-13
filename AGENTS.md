# CKB repository guidance

These defaults apply throughout the repository; closer `AGENTS.md` files add
subsystem guidance. Current user instructions take precedence. Keep enduring
repository knowledge here and task plans, history and evidence in project records.

## Scope and completion

Complete the requested outcome across its full scope. A PR review covers the
complete diff and affected call paths. Carry implementation work through refinement
and relevant verification; local improvements do not close broader objectives.
Resolve routine choices autonomously and ask only when missing information or
authority materially blocks progress. Retain unfinished objectives across
follow-ups unless the user replaces them.

Preserve existing work and keep edits and commits within scope. Inspect Git state
before changing it. Commit authorized changes after refinement and verification,
before reporting completion; do not wait for another prompt. Push, publish, merge
and release need explicit authorization; authorization already given remains valid.

## Engineering

Choose the simplest coherent design that preserves the required behavior and
resource bounds. Weigh maintainability, readability and performance together.
Use types and ownership to express real invariants; avoid duplicated state and
checks needed only to maintain it. Trace contracts through their producers and
consumers, including failure, cancellation and shutdown paths. Keep lock order,
ownership and cleanup clear across concurrent work.

Refine every change repeatedly before delivery, reconsidering the model as well
as its expression. Remove unnecessary states, layers, conventions and superseded
routes across complete call paths. Continue while concrete improvements remain;
a single pass or passing tests does not finish refinement. Avoid churn between
equivalent forms or moving complexity elsewhere.

CKB supports upgrades of existing nodes, including old configuration files.
Node downgrade compatibility is out of scope; unpublished intermediate designs
do not create compatibility obligations.

## Validation and delivery

Use the [Makefile](Makefile) for feature and lint conventions, selecting checks
for changed behavior and affected dependencies. Use Nextest's process isolation
for unit tests. Serialize builds sharing a Cargo graph and isolate test ports and
data directories. Reuse passing checks on unchanged relevant inputs; broaden or
repeat them when changes, failures or unresolved risks justify it. Documentation
edits need content and link checks, not Rust rebuilds.

Test meaningful failure paths and rejection when changing a gate. Establish
concurrency ordering with observable events; timing or stress alone is not proof.
Format changed Rust, run `git diff --check`, and resolve relevant failures and
warnings. Update affected tests and documentation with the implementation.

For long work, keep one concise checkpoint of the objective, constraints, source,
evidence, live processes and remaining work. Verify relevant source and process
identities before resuming. Report the outcome and actual validation with enough
source, command and configuration detail to assess it. Disclose gaps and separate
measured performance from expected benefits.
