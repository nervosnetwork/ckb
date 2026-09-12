# CKB Repository Instructions

Repository defaults apply throughout the tree; closer `AGENTS.md` files add
subsystem guidance. Current user instructions take precedence over repository
and skill guidance. Keep enduring rules here and live plans in project artifacts.

## Scope and execution

- Read applicable instructions and inspect branch, HEAD, index and worktree before
  editing. Preserve user changes; keep edits and commits within the requested scope.
- Carry authorized work through implementation and relevant verification. Resolve
  routine choices from context; ask only when missing information or authority
  materially blocks progress, and continue independent work. Prepare a concrete,
  reviewable result before any required approval. If a file instruction blocks
  work, identify its exact source and explain why it applies.
- Incorporate user corrections while retaining unfinished objectives unless the
  user replaces them. Avoid destructive Git operations. Push, publish, merge and
  release require explicit authorization; existing authorization remains valid.
- When using parallel agents, assign independent questions and clear file ownership.
  Coordinate shared resources and inspect their evidence and combined changes;
  the coordinating agent remains responsible for the result.

## Engineering judgment

- Choose the simplest coherent design meeting correctness, security and declared
  compatibility. Weigh resource use, performance and maintainability together;
  explain material tradeoffs without treating current types or stages as requirements.
- CKB supports upgrading existing nodes, including reading old configuration files.
  Node downgrade compatibility is out of scope; unpublished intermediate designs
  do not create compatibility obligations.
- Trace changed behavior through responsible producers and consumers, across crate
  boundaries where needed. For structural work, map the full requested subsystem's
  responsibilities and dependencies, including interfaces, tests and documentation;
  compare plausible alternatives against costs and acceptance evidence. Scale this
  investigation to the change, and distinguish necessary integration from unrelated
  cleanup. Do not substitute a few local improvements for the requested outcome.
- Separate source facts, hypotheses and conclusions. Check competing explanations
  before accepting a root cause. Fix responsible behavior, handle expected failures
  and verify trust-boundary assumptions. Justify unsafe code, termination and added
  recovery or coordination.
- Keep ownership, resource bounds and cleanup explicit through actual allocation
  and transfer paths. Concurrent changes must account for lock order, waiting,
  cancellation and shutdown, including dependencies needed to make progress.
- Refine every change before delivery through repeated semantic and subtractive
  review. Revisit the model as well as its expression; remove unnecessary states,
  layers and conventions across complete call paths, preserving contracts and
  resource bounds. Continue while concrete improvements remain; a single pass or
  passing tests does not finish refinement. Avoid churn between equivalent forms.

## Validation

- Choose checks for the changed behavior and affected dependencies, following the
  Makefile's feature and lint conventions. Its aggregate targets are entry points,
  not a mandatory sequence. Use Nextest's process isolation for unit tests.
- Test changed behavior and meaningful failure paths, including rejection when
  changing a gate. Use observable events to establish concurrency ordering;
  diagnostic traces, sleeps and stress alone do not prove correctness.
- Reuse passing checks on unchanged relevant inputs. A stricter check can cover a
  weaker one with matching scope and features; do not duplicate compiler checks
  around Clippy. Expand to workspace or full integration runs for shared-boundary
  risk, failures or unresolved concerns, and stop when acceptance evidence is met.
  Documentation-only changes need content/link checks, not Rust rebuilds.
- Serialize builds sharing a Cargo graph; isolate test ports and data directories.
  Format changed Rust, run `git diff --check`, and investigate warnings or failures.
  Record source, command, configuration, scope and outcome; disclose validation gaps.

## Delivery and continuity

- Integrate changes into the current code, tests and documentation. Rewrite the
  affected explanation as a coherent whole and remove superseded routes and
  references within scope. Keep development history in project records. Stage only
  intended files and keep commits reviewable.
- For long work, keep one concise project-owned checkpoint of the objective,
  constraints, source/process identities, results and remaining work. Update it at
  meaningful changes; avoid duplicate reports, repeated audits and routine status
  rewrites. Verify live processes before restarting work; retrieve history by question.
- Report the outcome, reason, evidence and limits in concise, plain language.
  Completion requires the requested behavior to work; reports and delegation alone
  do not establish it. Separate measured performance from expected benefits.
