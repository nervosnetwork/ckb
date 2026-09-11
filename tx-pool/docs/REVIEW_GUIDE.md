# Reviewing the transaction pool

Use this guide to trace current contracts through source and representative tests.
Read [architecture](ARCHITECTURE.md) for design and ownership and
[maintenance](MAINTENANCE.md) for changes and diagnosis. Performance measurement
has its own [protocol](BENCHMARK.md) and [report](PERFORMANCE.md).

## Trace the changed behavior

Read the [architectural core](ARCHITECTURE.md#the-architectural-core), then follow
the changed request through these boundaries. Queued progress can commit several
owner transitions before acceptance; direct local requests share policy/commit
without following every queued phase.

| Boundary | Review question |
|---|---|
| [Controller](../src/service/controller.rs) and [ingress](../src/authority/ingress.rs) | Are inputs bounded, source permissions preserved and caller completion stated correctly? |
| [Worker](../src/authority/service/execution.rs) and [jobs](../src/authority/jobs.rs) | Who owns each job, VM state, result and reservation through suspension, settlement and cancellation? Can block-dependent reconciliation and publication still progress? |
| [Membership](../src/authority/membership.rs) and [chain](../src/authority/chain.rs) | Which original observations justify both the decision and any rejection? |
| [Apply](../src/authority/store/apply.rs) and [budget](../src/authority/budget.rs) | Does complete preflight cover the exact owner, index, quota and effect changes before mutation? |
| [Outbox](../src/authority/notice.rs) | Are required obligations retained through waiting/failure and published in order outside guards? |
| [Template](../src/authority/template.rs), [query](../src/authority/query.rs), [persistence](../src/persisted.rs) | Does each consumer validate its actual sources and avoid inventing accepted proof or completion? |

Follow producers and consumers across the affected crate boundary. A current
caller proves use, not that a representation or extra coordination stage is
necessary. Review reorg, clear and shutdown alongside the ordinary path when they
can invalidate or retire the same owned data.

## Representative checks

| Contract | Tests to inspect |
|---|---|
| Real receive, canonical VM, commit, publication and joined service | [execution.rs](../src/authority/tests/execution.rs), `remote_receive_resolve_verify_commit_publish_and_join` |
| Independent and shared read-only work overlap inside final production cuts | [concurrency.rs](../src/authority/tests/concurrency.rs), `*_hold_final_commit_cuts_together` and `independent_remote_jobs_overlap_*` |
| Original reads reject ABA; failed multi-owner Apply is atomic | [contracts.rs](../src/authority/tests/contracts.rs), `original_owner_identity_*`, `a_read_set_cannot_replace_*`, `rejected_multi_owner_edit_*` |
| Complete phase/source changes and composed admission, RBF, recovery, reorg and clear preserve every population | [state_transitions.rs](../src/authority/tests/state_transitions.rs), exhaustive phase/source pairs and the real-planner sequence; independent projection and account-routing expectations, with the shared single-owner charge formula stated explicitly |
| Necessary input/dep premises, complete replacement effects, refusal rollback and preview isolation | [decision contracts](architecture/COMMIT.md#original-reads-and-intended-writes) map policy to independent mutation/interposition checks |
| Both orderings, peer eligibility, active quotas and unique queue selection | [contracts.rs](../src/authority/tests/contracts.rs) and [queue.rs](../src/authority/tests/queue.rs) |
| Serialized-size compatibility, execution room for small pools, proportional larger limits and overflow rejection | [budget.rs](../src/authority/tests/budget.rs); [configuration conversion](../../util/app-config/src/legacy/tx_pool.rs) and [bundled network configs](../../util/app-config/src/tests/app_config.rs) |
| One detached cell shared across input/dependency roles remains charged and fits its final owner | [verification.rs](../src/authority/tests/verification.rs), `resolution_shares_one_detached_cell_across_input_and_dependency_roles` |
| RBF exact fees, shared descendant accounting, conditional reads and capacity rejection | [membership.rs](../src/authority/tests/membership.rs) |
| Complete graph totals and batched eviction rank changes | [membership_aggregates.rs](../src/authority/tests/membership_aggregates.rs), exhaustive five-node DAGs plus limit/cycle/overflow refusal; [membership_trim.rs](../src/authority/tests/membership_trim.rs), shared-ancestor changes before the next victim choice |
| Canonical resolution, VM cache/rules, since/maturity, exact declared cycles and initial-load refusal | [pool verification](../src/authority/tests/verification.rs); [nonzero cellbase maturity](../../test/src/specs/tx_pool/cellbase_maturity.rs), rejection before the epoch boundary and successful same-transaction retry through commit; [cache identity](../../verification/src/tests/cache.rs) and [contextual block checks](../../verification/contextual/src/tests/contextual_block_verifier.rs), including fresh proof publication and task completion before the assume-valid negative assertion |
| Peer revocation rejects stale workers and late cohort changes | [ingress_contracts.rs](../src/authority/tests/ingress_contracts.rs) |
| FIFO, fixed ready-prefix selection, per-batch release, endpoint failure and cancellation | [notice.rs](../src/authority/tests/notice.rs) |
| Relay prefix allocation, reset ordering and waiting-parent reconstruction | [relay.rs](../src/authority/tests/relay.rs) and `public_relay_batch_drain_keeps_raw_reset_order_before_waiter_reconstruction` in [execution.rs](../src/authority/tests/execution.rs) |
| Queue-only changes stay quiet; dependency, capacity and lifecycle changes wake required work | [contracts.rs](../src/authority/tests/contracts.rs), maintenance/worker notification tests |
| Chain invalidation, bounded recovery and proposal provenance | [pool chain tests](../src/authority/tests/chain.rs), [proposal projection tests](../../util/proposal-table/src/tests.rs), and [reorg integration](../../test/src/specs/tx_pool/reorg_recovers_dependent.rs), which commits transactions before detaching and after recovery |
| Reliable reorg delivery, clear ordering and compact bounded public inputs | [controller.rs](../src/service/tests/controller.rs) |
| Block priority gates queued/direct computation, preserves chain progress and restores after backlog/unwind | [execution.rs](../src/authority/tests/execution.rs), pause/suspended tests; [chain priority tests](../../chain/src/tests/verification_priority.rs) |
| CPFP, complete ancestors and read-before-spend ordering | [packing.rs](../src/authority/tests/packing.rs); [packing_ordering.rs](../src/authority/tests/packing_ordering.rs), induced subsets versus fresh graphs and complete package drops; [packing_graph.rs](../src/authority/tests/packing_graph.rs), stateless five-node policy/descendant-set oracles, overlapping 1000-entry boundary, residual-child/overflow budget gates, duplicate canonical inputs, source replacement, proposal transitions and retired allocation/payload lifetimes; [proposal lookup](../../util/proposal-table/src/tests.rs) and [borrowed protocol fields](../../util/types/src/core/tests/views.rs) |
| Exact mandatory byte fit, selected-owner ABA, mandatory-payload reuse and stale template refusal | [template_driver.rs](../src/authority/tests/template_driver.rs) |
| DAO memo capacity, hot-entry retention, tip invalidation and fresh in-block overlay | [block-assembler tests](../src/block_assembler/tests/mod.rs) |
| Accepted-only public proofs, full-hash identity and history visibility | [query.rs](../src/authority/tests/query.rs) and public-query execution tests |
| Root metadata content/version identity, per-attempt load receipt and thread-exit cleanup | [program-cache tests](../../script/src/program_cache.rs); [VM budget tests](../../script/src/verify/tests/active_budget.rs) |
| Callback reads, rejected mutation reentry and faulted-save preservation | [execution.rs](../src/authority/tests/execution.rs) and controller tests |
| Exhaustive rejection policy and preserved legacy normalization | [Reject tests](../../util/types/src/core/tx_pool.rs) and [configuration tests](../../util/app-config/src/legacy/tx_pool.rs) |

Use observable barriers and outcomes to establish concurrency ordering. Sleeps,
stress, diagnostic task counts and passing aggregate totals alone do not prove
these contracts. Test scopes overlap and must not be added into a unique total.

On macOS, a Nextest `LEAK` report means an output pipe stayed open after its test
process exited. Concurrent process startup can pass that pipe to a sibling test;
this was observed with Nextest 0.9.143 through kernel pipe identities and inherited
descriptors. Use `--test-threads 1` on this affected runner to serialize test
processes. Each test remains isolated, and its own Rust threads and Tokio tasks
still exercise the concurrency under test. Keep output capture and the normal
leak timeout enabled: a deliberately retained descendant pipe must still be
detected. Diagnose new reports through pipe ownership, following
[Nextest's leak guidance](https://nexte.st/docs/features/leaky-tests/).

## Maintain test value

Keep a case's original trigger order and terminal contract when improving its
assertions. Pending, gap and proposed collision cases may all reject the same
submission yet exercise different later proposal histories. Reorg tests that
manually commit, naturally select or omit an uncle also protect different paths.
Check the introducing change when names or old comments no longer explain the
scenario. Similar setup is insufficient evidence for deletion.

An empty queue or aggregate count can already hold before asynchronous work
starts. Wait for the submitted transaction's exact status, rejection or requested
parent hashes before checking cleanup. Preserve downstream waits until the actual
consumer has an observable acknowledgement: a visible ban does not establish that
the relayer consumed its filter reset. Within unit tests, poll the real acquisition
or notification future to establish Pending/Ready instead of relying on a yield
or elapsed delay. Keep channel receivers alive when testing Full versus Closed.

Use one immutable snapshot within a parameter matrix while retaining fresh Store,
queue and reservation state where each case requires isolation. Consolidation must
preserve each original input and independent expected result. Exact capacity and
output-index boundaries need both the successful endpoint and adjacent refusal.

For mutation checks, prelist a bounded set of relevant gates and use isolated
source plus Nextest. Recheck survivors against the full relevant suite before
calling them coverage gaps; distinguish valid-state equivalence, compilation
failure, timeout and a missed behavioral change. Removing a redundant test needs
contract evidence; matching detection of selected mutations is supporting evidence,
not proof that the tests are interchangeable. Label hand-seeded perturbations
separately from mutations generated by the tool.

The real-node [pool specs](../../test/src/specs/tx_pool) protect relay, mining and
restart behavior beyond private authority fixtures. Enumerate active names with
`ckb-test --list-specs` and include relevant RPC consumers such as
[TxPoolEntryStatus](../../test/src/specs/rpc/get_pool.rs) and
[truncate](../../test/src/specs/rpc/truncate.rs), [transaction relay](../../test/src/specs/relay/transaction_relay.rs),
[compact-block pool reads](../../test/src/specs/relay/compact_block.rs) and
[proposal-batch mining](../../test/src/specs/mining/fee.rs) when their boundary is affected.
A Spec implementation or unused method is not executed coverage. Confirm registration
and call paths when reviewing old cases. The shared [commit assertion](../../test/src/specs/tx_pool/utils.rs)
checks the exact transaction through Pending, Gap, Proposed and Committed, including
its expected commit height; both since and cellbase-maturity cases use it.

## Check resource composition

Unique-cell precharge, subsequent copies and final retained-owner checks must
compose: local bounds alone cannot prevent an earlier allocation peak. Review
the complete allocating path:

1. Identify whether each boundary counts unique payloads, semantic occurrences,
   reference-array capacity, bytes, edges or concurrent producers.
2. Name the reservation active before allocation, including source/destination
   overlap and replaced values. A later check cannot bound an earlier peak.
3. Follow sharing and detachment through actual producers and consumers. Include
   trusted bypasses, suspended residency, rejection, stale work, cancellation and cleanup.
4. Keep a production-path regression joining the critical facts. State logical
   bounds separately from allocator behavior and process RSS.

Full graph/capture scratch, database cache, allocator overhead and VM internals
are separate from the per-job payload budget. The configured pool limits are not
a universal process-RSS cap. [Architecture resource bounds](architecture/EXECUTION.md#resource-and-lifetime-bounds)
describe the composed populations and their assumptions.

## Assess value and limits

Compare the current [commit optimizations](architecture/COMMIT.md#current-optimizations)
and [execution optimizations](architecture/EXECUTION.md#current-optimizations)
against their prerequisites and costs. Line counts, local passes and fewer
coordination stages alone do not establish minimality or universal superiority.
Bind executed checks to the source they tested. [Benchmarking](BENCHMARK.md)
binds performance decisions to frozen inputs and workload scope.

## Development and review method

Start with a concrete behavior and its failure modes. Map the request producer,
source of truth, decision premises, commit and consumer before choosing a type or
stage. Compare plausible alternatives by the coordination, retained data and
maintenance work they actually remove. Then make the smallest coherent change
that preserves the complete contract, including refusal and cleanup.

Use `rg` and Git diffs to follow definitions and callers across pool, sync, chain,
RPC, shared setup and canonical verification. Compiler and Clippy checks expose
interface mistakes; event-driven production-path tests establish ordering and
resource release. Makefile aggregates check the wider dependency boundary.
For representation changes, compare source before/after and inspect generated
code or debug layout only where a concrete allocation or protected-work question
requires it. Assembly and line counts do not establish end-to-end value.

For performance work, use the [profiling workflow](PROFILING.md#investigate-a-performance-problem)
to form a causal hypothesis, isolate one material change and test it with a
frozen comparison. Keep raw failures and adverse CPU or memory outcomes. Review
the retained mechanism as well as its measured gain; an optimization that breaks
atomicity or cleanup is not eligible for a timing decision.

A change is ready for review when another maintainer can explain its ownership,
ordering and bounds from the responsible code, run its meaningful regression,
reproduce any performance claim from immutable inputs, and find all relevant
limits without reconstructing development history. Check source links, public
configuration/RPC documentation and migration notes whenever those contracts move.
